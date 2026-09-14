#![allow(dead_code)]
//! Byte-exact rebuild oracle for GenAttr (`LID4nnnn`) blocks.
//!
//! Goal (writer correctness, provable offline without a car): re-encode every GenAttr block's
//! *stream bytes* from the values `decode_block` produces and require the result to be byte-for-byte
//! identical to the stock block. If our encoders reproduce what the *authoring tool* produced — and
//! the car's decoders round-trip our decoded values identically — then emitting OSM data through the
//! same encoders yields device-faithful columns. No on-car test needed.
//!
//! Layout (`NLGeneralAttributeBlock::SetDataBlock` @0xe09b60): `[u16 num_desc][num_desc*12B
//! {u16 kind, u16 code, u32 off, u32 param}]...` where `off` is block-relative, rows are ascending
//! by `off`, and row `i`'s *byte* span is `[off[i], off[i+1])` (the last ends at the block end).
//! Rows sharing an `off` make the earlier row a zero-length (empty) view; the last of the tie owns
//! the bytes. `param` is the logical decoded-element count, decoupled from byte length. We rebuild
//! each row's bytes from its decoded values and require byte-equality to the stock block. The
//! descriptor table and any inter-region padding are preserved verbatim (device ignores them).

use super::*;

fn parse_descs(b: &[u8], bs: usize, num: usize) -> Vec<(u16, u32, u32, u32)> {
    (0..num).map(|j| {
        let p = bs + 2 + 12 * j;
        (u16(b, p) as u16, u16(b, p + 2), u32(b, p + 4), u32(b, p + 8))
    }).collect()
}

/// Decode a numeric stream to *region exhaustion* (bijection: VLE/Simple9/delta/read-to-eof), used by
/// the rebuild oracle only. The reader's `decode_block` clamps to `param` (the device's logical count);
/// the oracle needs every byte the author wrote, so it reads until the region ends.
fn decode_u32_region(b: &[u8], code: u32, s: usize, en: usize) -> Vec<u32> {
    decode_u32(b, code, s, en, 64_000_000) // read to region EOF (bijection), not a byte- or param-clamped count
}
fn bitfield_region(b: &[u8], code: u32, s: usize, en: usize) -> Vec<bool> {
    bitfield(b, code, s, en, (en - s) * 8)
}
#[allow(dead_code)]
fn _unused_bitfield_region() { let _ = bitfield_region; }


/// Inverse of `Cur::simple9` (decoder @0xcdb3bc / @0xcdc908, mode in bits 28..32). Greedy: emit the
/// largest-count mode whose value window fits its bit width (mode 9 = fewest values, widest).
pub(crate) fn encode_simple9(v: &[u32]) -> Vec<u32> {
    const MODES: [(u32, usize, usize); 9] = [
        (1, 28, 1), (2, 14, 2), (3, 9, 3), (4, 7, 4), (5, 5, 5),
        (6, 4, 7), (7, 3, 9), (8, 2, 14), (9, 1, 28),
    ];
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < v.len() {
        let mut pick = MODES[8];
        for &m @ (_, cnt, bits) in &MODES {
            if i + cnt <= v.len() && v[i..i + cnt].iter().all(|&x| (x as u64) < (1u64 << bits)) {
                pick = m;
                break;
            }
        }
        let (_, cnt, bits) = pick;
        let mut word: u32 = pick.0 << 28;
        for k in 0..cnt {
            word |= (v[i + k] & ((1u32 << bits) - 1)) << (bits * k);
        }
        out.push(word);
        i += cnt;
    }
    out
}

/// Re-encode a numeric stream (`decode_u32`). `None` = authoring packing not yet pinned.
pub(crate) fn encode_numeric(v: &[u32], code: u32) -> Option<Vec<u8>> {
    let mut o = Vec::new();
    match code {
        0x11 => for &x in v { o.extend_from_slice(&x.to_le_bytes()) },
        0x14 => for &x in v { o.extend(vle_encode(x)) },
        0x16 => { let mut acc = 0i64; for &x in v { let d = (x as i64) - acc; acc = x as i64; o.extend(vle_encode(d as u32)) } }
        0x18 => for &w in &encode_simple9(v) { o.extend(w.to_le_bytes()) },
        0x13 => {
            let top = *v.last().unwrap_or(&0) as usize;
            let mut by = vec![0u8; top / 8 + 1];
            for &idx in v { by[idx as usize / 8] |= 1 << (idx as usize % 8) }
            o = by;
        }
        c @ (0x12 | 0x15 | 0x17) => {
            // boundary stream: each ascending-by-1 run ends at a boundary; 0x12 stores it raw-u32,
            // 0x15/0x17 as cumulative delta-VLE (matching the run/bitmap-expansion decoders).
            let mut prev = 0u32;
            let mut i = 0usize;
            while i < v.len() {
                let mut j = i;
                while j + 1 < v.len() && v[j + 1] == v[j].wrapping_add(1) { j += 1 }
                let end = v[j];
                let bnd = end.wrapping_sub(prev);
                if c == 0x12 { o.extend_from_slice(&bnd.to_le_bytes()) } else { o.extend(vle_encode(bnd)) }
                prev = end;
                i = j + 1;
            }
        }
        _ => return None,
    }
    Some(o)
}

/// Re-encode a bitmap stream (`bitfield`).
pub(crate) fn encode_bitmap(bits: &[bool], code: u32) -> Option<Vec<u8>> {
    let mut o = Vec::new();
    match code {
        0x01 => {
            let mut by = vec![0u8; (bits.len() + 7) / 8];
            for (i, &bit) in bits.iter().enumerate() { if bit { by[i / 8] |= 1 << (i % 8) } }
            o = by;
        }
        0x02 => { let mut acc = 0u32; for (i, &b) in bits.iter().enumerate() { if b { let p = i as u32; o.extend(vle_encode(p - acc)); acc = p } } }
        0x03 => { let mut acc = 0u32; for (i, &b) in bits.iter().enumerate() { if !b { let p = i as u32; o.extend(vle_encode(p - acc)); acc = p } } }
        _ => return None,

    }
    Some(o)
}

pub enum Enc {
    Numeric,
    Bitmap,
}

/// Re-encode row (`values`/`bits`, `code`). Returns encoded bytes or `None` for unpinned codes.
pub fn encode_row(values: &[u32], bits: &[bool], code: u32) -> Option<Vec<u8>> {
    match code {
        0x01 | 0x02 | 0x03 => encode_bitmap(bits, code),
        _ => encode_numeric(values, code),
    }
}

#[derive(Debug)]
pub struct Mismatch {
    pub block: usize,
    pub row: usize,
    pub kind: u16,
    pub code: u32,
    pub param: u32,
    pub off: u32,
    pub regpos: usize,
    pub detail: String,
}

/// Rebuild block `bi`. Returns (rebuilt bytes, first mismatch). Rebuilt == original iff None.
pub fn rebuild_block(g: &GenAttrIndex, b: &[u8], bi: usize) -> (Vec<u8>, Option<Mismatch>) {
    let e = g.blocks[bi];
    let bs = e.block_off as usize;
    let be = if bi + 1 < g.blocks.len() { g.blocks[bi + 1].block_off as usize } else { b.len() };
    if bs >= b.len() { return (Vec::new(), Some(Mismatch { block: bi, row: 0, kind: 0, code: 0, param: 0, off: e.block_off, regpos: 0, detail: "block_off past EOF (skip: device also can't read it)".into() })); }
    let be = be.min(b.len());
    let orig = if be > bs { &b[bs..be] } else { &[][..] };
    let num = u16(b, bs) as usize;
    let descs = parse_descs(b, bs, num);
    let tbl_end = 2 + 12 * num;

    let mut out: Vec<u8> = orig[..tbl_end.min(orig.len())].to_vec(); // verbatim [u16 num][table]
    for i in 0..num {
        let off = descs[i].2 as usize;
        let end = if i + 1 < num { descs[i + 1].2 as usize } else { orig.len() };
        let region = &orig[off.min(orig.len())..end.max(off).min(orig.len())];
        let (code, kind) = (descs[i].1, descs[i].0);
        let is_bmp = matches!(code, 0x01 | 0x02 | 0x03);
        let vals = if is_bmp { Vec::new() } else { decode_u32_region(b, code, bs + off, bs + end) };
        let nbits = (descs[i].3 as usize).min(64_000_000);
        let bits = if is_bmp { bitfield(b, code, bs + off, bs + end, nbits) } else { Vec::new() };

        let enc = match encode_row(&vals, &bits, code) {
            Some(e) => e,
            None => {
                let det = format!("unpinned code {:#03x} (kind {:#04x}, {} bytes)", code, kind, region.len());
                return (out, Some(Mismatch { block: bi, row: i, kind, code, param: descs[i].3, off: descs[i].2, regpos: off, detail: det }));
            }
        };
        if enc.len() != region.len() {
            let det = format!("len {} != reg {} (code {:#03x} kind {:#04x} flags {:#x})", enc.len(), region.len(), code, kind, kind & 0xf000);
            return (out, Some(Mismatch { block: bi, row: i, kind, code, param: descs[i].3, off: descs[i].2, regpos: off, detail: det }));
        }
        if let Some(p) = region.iter().zip(&enc).position(|(x, y)| x != y) {
            let det = format!("byte @{} stock={:#02x} our={:#02x} (code {:#03x} kind {:#04x})", p, region[p], enc[p], code, kind);
            return (out, Some(Mismatch { block: bi, row: i, kind, code, param: descs[i].3, off: descs[i].2, regpos: off, detail: det }));
        }
        out.extend_from_slice(&enc);
    }
    (out, None)
}

pub fn rebuild_all(b: &[u8]) -> (usize, Option<Mismatch>) {
    let g = read_gen_attr(b).expect("gen_attr");
    let mut ok = 0usize;
    for bi in 0..g.blocks.len() {
        let (_, mm) = rebuild_block(&g, b, bi);
        match mm {
            Some(m) if m.detail.starts_with("block_off past EOF") => ok += 1,
            Some(m) => return (ok, Some(m)),
            None => ok += 1,
        }
    }
    (ok, None)
}
