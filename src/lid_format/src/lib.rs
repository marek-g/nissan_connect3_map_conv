//! Reader for Bosch TravelMap LID address name-list files (`LID*.DAT`).
//!
//! These are the columnar ASF `NLAsfBlock` files that the car's address search
//! (`LISA_tclAddressSearch::bGetNames` -> `NLProcessor`) walks. A name-list is a
//! (minimized) trie/DAWG: per-node out-degree (CSR/BFS) + per-node edge-labels
//! (a byte blob, offset-indexed) + a charset, with per-element columns for the
//! WGS84 position (`NLPositionAttrVector`, origin-relative PAU deltas) and the
//! belonging-name (city) link. This crate decodes that into a flat gazetteer.
//!
//! Reverse-engineered ground truth (this repo): container/block/descriptor framing,
//! the `NLStandardDecoder` alphabet, `NLAsfBlock::ProcessNode` (BFS tree),
//! `NLPositionAttrVector::Decode`, `NLBlockLinkAttrVector::Decode`,
//! `NLBlock::ProcessNode`. See LID_format.md (ASF section).

mod rebuild;
pub mod write;

/// PAU = "position angle unit": deg * 2^31 / 180 (signed 32-bit).
pub const PAU: f64 = (1i64 << 31) as f64 / 180.0;

pub fn pau_to_deg(v: i32) -> f64 {
    v as f64 / PAU
}

fn u16(b: &[u8], o: usize) -> u32 {
    if o + 2 > b.len() {
        0
    } else {
        u16::from_le_bytes([b[o], b[o + 1]]) as u32
    }
}
fn u32(b: &[u8], o: usize) -> u32 {
    if o + 4 > b.len() {
        0
    } else {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }
}

fn put_u16(v: Vec<u8>, x: u16) -> Vec<u8> {
    let mut v = v;
    v.extend_from_slice(&x.to_le_bytes());
    v
}
fn put_u32(v: Vec<u8>, x: u32) -> Vec<u8> {
    let mut v = v;
    v.extend_from_slice(&x.to_le_bytes());
    v
}

/// Encode one integer with the custom `ReadVle` (`00cdb218`) stream format (inverse of `Cur::vle`).
/// value = final_byte + 128*(bijective-base-128 continuation chunks); continuations are high-bit-set
/// bytes emitted most-significant first, then a high-bit-clear final byte.
pub(crate) fn vle_encode(v: u32) -> Vec<u8> {
    let mut cont: Vec<u8> = Vec::new();
    let mut w = v / 128;
    while w > 0 {
        w -= 1;
        cont.push(0x80 | (w % 128) as u8); // low-weight chunk first
        w /= 128;
    }
    cont.reverse(); // stream order: highest weight first
    cont.push((v % 128) as u8); // final byte (high bit clear)
    cont
}

// ---- Simple9 (mode nibble in bits 28..32). mode -> (count, bits-per-value). ----
// From DecodeSimple9<u32> @0xcdb3bc / <u16> @0xcdc908 (values extracted LSB-first).
fn simple9_mode(m: u32) -> Option<(usize, usize)> {
    Some(match m {
        1 => (28, 1),
        2 => (14, 2),
        3 => (9, 3),
        4 => (7, 4),
        5 => (5, 5),
        6 => (4, 7),
        7 => (3, 9),
        8 => (2, 14),
        9 => (1, 28),
        _ => return None,
    })
}

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
    end: usize,
}
impl<'a> Cur<'a> {
    fn eof(&self) -> bool {
        self.p >= self.end
    }
    fn vle(&mut self) -> u32 {
        // ReadVle @0xcdb218: custom LEB; continuation byte adds 128*prev,
        // (acc = (byte-0x7f) + prev*128), final byte yields byte + prev*128.
        let start = self.p;
        let mut acc: u32 = 0;
        loop {
            let mult = acc.wrapping_mul(0x80);
            if start + 5 <= self.p || self.eof() {
                return acc;
            }
            let by = self.b[self.p];
            self.p += 1;
            acc = (by as u32).wrapping_sub(0x7f).wrapping_add(mult);
            if by & 0x80 == 0 {
                return (by as u32).wrapping_add(mult);
            }
        }
    }
    fn ru32(&mut self) -> u32 {
        let v = u32(self.b, self.p);
        self.p = (self.p + 4).min(self.end.max(self.p));
        if self.p > self.end {
            self.p = self.end;
        }
        v
    }
    fn ru16(&mut self) -> u32 {
        let v = u16(self.b, self.p);
        self.p += 2;
        v
    }
    fn simple9(&mut self, n: usize, bits9: usize) -> Vec<u32> {
        let mut out = Vec::with_capacity(n);
        while out.len() < n && self.p + 4 <= self.end {
            let mut w = u32(self.b, self.p);
            self.p += 4;
            let mode = (w >> 28) & 0xf;
            let (cnt, mut bits) = match simple9_mode(mode) {
                Some(x) => x,
                None => break,
            };
            if mode == 9 && bits9 != 28 {
                bits = bits9;
            }
            let mask = (1u32 << bits) - 1;
            for _ in 0..cnt {
                if out.len() >= n {
                    break;
                }
                out.push(w & mask);
                w >>= bits;
            }
        }
        out
    }
}

/// Decode a numeric column with `NLStandardDecoder`/`NLValueListDecoder` alphabet.
fn decode_u32(b: &[u8], code: u32, start: usize, end: usize, n: usize) -> Vec<u32> {
    decode_u32_c(b, code, start, end, n).0
}

/// RE-harness probe: same as `decode_u32` but also returns the byte cursor after the last value.
#[doc(hidden)]
pub fn decode_u32_probe(
    b: &[u8],
    code: u32,
    start: usize,
    end: usize,
    n: usize,
) -> (Vec<u32>, usize) {
    decode_u32_c(b, code, start, end, n)
}

fn decode_u32_c(b: &[u8], code: u32, start: usize, end: usize, n: usize) -> (Vec<u32>, usize) {
    let mut c = Cur {
        b,
        p: start,
        end: end.min(b.len()),
    };
    let mut out = Vec::with_capacity(n.min(1_000_000));
    match code {
        0x11 => {
            for _ in 0..n {
                if c.eof() {
                    break;
                }
                out.push(c.ru32())
            }
        }
        0x14 => {
            for _ in 0..n {
                if c.eof() {
                    break;
                }
                out.push(c.vle())
            }
        }
        0x16 => {
            let mut acc = 0i64;
            for _ in 0..n {
                if c.eof() {
                    break;
                }
                acc += c.vle() as i64;
                out.push(acc as u32)
            }
        }
        0x18 => out = c.simple9(n, 28),
        0x13 => {
            // bitmask -> set-bit indices
            let mut i = 0usize;
            let mut idx = 0u32;
            while !c.eof() && i < n {
                let by = c.b[c.p];
                c.p += 1;
                for k in 0..8 {
                    if by & (1 << k) != 0 {
                        if i >= n {
                            break;
                        }
                        out.push(idx);
                        i += 1
                    }
                    idx += 1;
                }
            }
        }
        0x12 | 0x15 => {
            let mut i = 0usize;
            let mut val = 0u32;
            while !c.eof() && i < n {
                let cnt = if code == 0x12 { c.ru32() } else { c.vle() };
                while val < cnt && i < n {
                    out.push(val);
                    val += 1;
                    i += 1
                }
            }
        }
        0x17 => {
            let mut i = 0usize;
            let mut val = 0u32;
            let mut tgt = 0u32;
            while !c.eof() && i < n {
                tgt = tgt.wrapping_add(c.vle());
                while val < tgt && i < n {
                    out.push(val);
                    val += 1;
                    i += 1
                }
            }
        }
        _ => {
            for _ in 0..n {
                if c.eof() {
                    break;
                }
                out.push(c.ru32())
            }
        }
    }
    (out, c.p)
}

fn decode_u16(b: &[u8], code: u32, start: usize, end: usize, n: usize) -> Vec<u32> {
    let mut c = Cur {
        b,
        p: start,
        end: end.min(b.len()),
    };
    let mut out = Vec::with_capacity(n.min(1_000_000));
    match code {
        0x11 => {
            for _ in 0..n {
                if c.eof() {
                    break;
                }
                out.push(c.ru16())
            }
        }
        0x14 => {
            for _ in 0..n {
                if c.eof() {
                    break;
                }
                out.push(c.vle())
            }
        }
        0x16 => {
            let mut acc = 0i64;
            for _ in 0..n {
                if c.eof() {
                    break;
                }
                acc += c.vle() as i64;
                out.push((acc & 0xffff) as u32)
            }
        }
        0x18 => out = c.simple9(n, 16),
        _ => {
            for _ in 0..n {
                if c.eof() {
                    break;
                }
                out.push(c.ru16())
            }
        }
    }
    out
}

/// Decode a `NLBitfieldDecoder` bitmap of `n` bits.
pub(crate) fn bitfield(b: &[u8], code: u32, start: usize, end: usize, n: usize) -> Vec<bool> {
    bitfield_c(b, code, start, end, n).0
}

/// RE-harness probe: same as `bitfield` but also returns the byte cursor after the last read.
#[doc(hidden)]
pub fn bitfield_probe(
    b: &[u8],
    code: u32,
    start: usize,
    end: usize,
    n: usize,
) -> (Vec<bool>, usize) {
    bitfield_c(b, code, start, end, n)
}

fn bitfield_c(b: &[u8], code: u32, start: usize, end: usize, n: usize) -> (Vec<bool>, usize) {
    let mut bits = vec![false; n];
    let end = end.min(b.len());
    let mut cur = start;
    match code {
        0x01 => {
            for i in 0..n {
                let q = start + (i >> 3);
                if q >= end {
                    break;
                }
                bits[i] = b[q] >> (i & 7) & 1 != 0
            }
            cur = (start + (n + 7) / 8).min(end);
        }
        0x02 => {
            let mut c = Cur { b, p: start, end };
            let mut acc = 0u32;
            for _ in 0..n {
                if c.eof() {
                    break;
                }
                acc += c.vle();
                if (acc as usize) < n {
                    bits[acc as usize] = true
                }
            }
            cur = c.p;
        }
        0x03 => {
            bits = vec![true; n];
            let mut c = Cur { b, p: start, end };
            let mut acc = 0u32;
            for _ in 0..n {
                if c.eof() {
                    break;
                }
                acc += c.vle();
                if (acc as usize) < n {
                    bits[acc as usize] = false
                }
            }
            cur = c.p;
        }
        _ => {}
    }
    (bits, cur)
}

#[derive(Debug, Clone)]
pub struct Element {
    pub category: u16,
    pub name: String,
    pub x_pau: i32, // stored delta: absolute = queried-city (or file `origin`) + (x_pau, y_pau) (§12.5)
    pub y_pau: i32,
    pub has_pos: bool,
    pub belonging: u32, // parent-city element id *within the same block* (device id space); 0xffffffff = none.
                        // Stock POL name-lists set it nowhere (probe `belonging_probe`); kept for fidelity.
}

/// The whole decoded name-list of a `LID*.DAT` block-container.
pub struct NameList {
    pub element_count: u32,
    pub block_count: usize,
    pub elements: Vec<Element>,
    /// File position origin (PAU) = the `tNLHPosition` the sub-header carries (`hdr+0x10/+0x14`, gated by
    /// bit `0x0008_0000` of `hdr+0x0c`); street tiles use it only when no city context exists, street
    /// coordinates are otherwise relative to the *queried city* (see §12.5). `None` = "-1/-1" = the file
    /// carries no origins (POL city gazetteer `LID20000`).
    /// `hdr+0x0c`'s low bits (`0x13`/`0x01`/`0x1d`/`0` on stock POL) are file-flavor hints; the decoder
    /// only honors bit `0x0008_0000`.
    pub origin: Option<(i32, i32)>,
}

/// One `NLBlockTocEntry` of a GenAttr file (fileID+20000): the element index range a block
/// covers plus that block's file offset. Read by `NLGenAttrFile::DecodeSubHeader` (0xe0e3d0).
#[derive(Debug, Clone, Copy)]
pub struct GenAttrTocEntry {
    pub elem_start: u32, // NLInterval start (first street element this block describes)
    pub elem_end: u32,   // NLInterval end
    pub block_off: u32,  // block file offset; the last block runs to EOF
}

/// A parsed GenAttr (`LID4nnnn.DAT`) file index. Its HNR columns (per-street parity/ranges,
/// `NLGeneralAttributeBlock`) are NOT decoded here — only the block/element TOC, which is the
/// layer the name-list reader was incorrectly applying (and returning garbage on).
pub struct GenAttrIndex {
    pub element_count: u32,
    pub blocks: Vec<GenAttrTocEntry>,
}

/// GenAttr sub-header layout (`NLGenAttrFile::DecodeSubHeader` @0xe0e3d0), read at `hdr`:
/// `u32 elem_count(=this+0x80), u32 this+0x84, u32 toc_count(=this+0x88)`, then `toc_count ×
/// NLBlockTocEntry` of 3 `u32` each. Distinct from `NLNameList::LoadHeader`'s VLE/raw section table.
fn gen_attr_toc(b: &[u8]) -> Option<(u32, Vec<GenAttrTocEntry>)> {
    let n = b.len();
    let hdr = u32(b, 0x10) as usize;
    if hdr + 24 > n {
        return None;
    }
    // sub-header: `u32 elem_count, u32 this+4, u32 toc_count, u32 block_count`, then toc_count
    // NLBlockTocEntry {u32 elem_start, u32 elem_end, u32 block_off} (NLGenAttrFile 0xe0e3d0).
    let elem_count = u32(b, hdr);
    let toccount = u32(b, hdr + 8);
    if toccount < 2 || toccount > 1_000_000 || elem_count < 1 || elem_count > 0x7fff_ffff {
        return None;
    }
    let mut p = hdr + 16;
    let mut blocks: Vec<GenAttrTocEntry> = Vec::with_capacity((toccount as usize).min(n / 12));
    for i in 0..toccount {
        if p + 12 > n {
            break;
        }
        let (a, c, o) = (u32(b, p), u32(b, p + 4), u32(b, p + 8));
        p += 12;
        // element ranges must tile contiguously & ascending (first starts at 0, ends at elem_count-1)
        let contiguous = i == 0 || a == blocks[i as usize - 1].elem_end + 1;
        if !contiguous || a > c || c >= elem_count {
            break;
        }
        blocks.push(GenAttrTocEntry {
            elem_start: a,
            elem_end: c,
            block_off: o,
        });
    }
    // accept only a full tiling of [0, elem_count) — the strong signal that distinguishes GenAttr
    if blocks.is_empty()
        || blocks[0].elem_start != 0
        || blocks.last().unwrap().elem_end != elem_count - 1
    {
        return None;
    }
    Some((elem_count, blocks))
}

/// True if `b` is a GenAttr (`NLGenAttrFile`) file: its block-offset vector is a `NLBlockTocEntry`
/// TOC whose element ranges tile `[0, elem_count)` (validated), not a name-list section table.
pub fn is_gen_attr(b: &[u8]) -> bool {
    b.len() >= 0x100 && gen_attr_toc(b).is_some()
}

/// Parse a GenAttr file index. Use this for `LID4nnnn.DAT` (fileID+20000), never `read`.
pub fn read_gen_attr(b: &[u8]) -> Result<GenAttrIndex, String> {
    gen_attr_toc(b)
        .map(|(element_count, blocks)| GenAttrIndex {
            element_count,
            blocks,
        })
        .ok_or_else(|| "not a GenAttr file (TOC element ranges do not tile)".into())
}

/// One decoded descriptor stream of a `NLGeneralAttributeBlock`. `NLGeneralAttributeBlock::SetDataBlock`
/// (@0xe09b60) reads `[u16 num_desc]` then `num_desc × {u16 kind, u16 code, u32 off, u32 param}`,
/// and dispatches `kind & 0xfff` to an attribute-vector slot and `kind & 0xf000` to that vector's
/// sub-stream (identical scheme to the name-list columns). The raw stream is decoded with the same
/// codecs (Simple9 / VLE / bitmap / delta) used by the name-list reader.
#[derive(Debug, Clone)]
pub struct GenAttrStream {
    pub col: u32, // kind & 0xfff  (0xc01..0xc14 = HNR attribute vectors; 0x001..0x005 = CellId)
    pub flags: u32, // kind & 0xf000 (0 = values, 0x4000 = secondary, 0x8000 = tertiary sub-stream)
    pub code: u32, // codec id: 0x01/0x02/0x03 = bitmap; 0x11 raw; 0x14 Simple9; 0x16 delta; 0x18 Simple9-u32
    pub param: u32, // element/value count this stream decodes to (or bit count for a bitmap)
    pub values: Vec<u32>,
    pub bits: Vec<bool>,
}

/// A decoded GenAttr block: its element range, file offset, and every descriptor stream.
#[derive(Debug, Clone)]
pub struct GenAttrBlock {
    pub elem_start: u32,
    pub elem_end: u32,
    pub block_off: u32,
    pub streams: Vec<GenAttrStream>,
}

impl GenAttrIndex {
    /// Decode the internal structure of block `bi` (descriptors + attribute-vector streams).
    /// This is the `NLGeneralAttributeBlock` layer beneath the TOC that `read_gen_attr` exposes.
    pub fn decode_block(&self, b: &[u8], bi: usize) -> Result<GenAttrBlock, String> {
        let e = *self.blocks.get(bi).ok_or("block index out of range")?;
        let n = b.len();
        let bs = e.block_off as usize;
        let be = if bi + 1 < self.blocks.len() {
            self.blocks[bi + 1].block_off as usize
        } else {
            n
        };
        if bs + 2 > n {
            return Err("truncated GenAttr block".into());
        }
        let num_desc = u16(b, bs) as usize;
        let mut descs: Vec<Desc> = Vec::with_capacity(num_desc);
        let mut p = bs + 2;
        for _ in 0..num_desc {
            if p + 12 > be {
                break;
            }
            let k = u16(b, p);
            descs.push(Desc {
                kind: k & 0xfff,
                flags: k & 0xf000,
                code: u16(b, p + 2),
                off: u32(b, p + 4),
                param: u32(b, p + 8),
            });
            p += 12;
        }
        // Device-exact stream span (`SetDataBlock` @0xe09b60): each descriptor's byte region is
        // [off[i], off[i+1]) in *table row order* (equal-`off` alias => the earlier row's length is 0,
        // the later carries the bytes); the last row ends at the block end. `param` is the logical
        // decoded-element count, decoupled from byte length. This is NOT "next strictly-greater off"
        // (which over-reads the first descriptor of an equal-`off` pair).
        let offs: Vec<u32> = descs.iter().map(|d| d.off).collect();
        let span_end = |i: usize| -> usize {
            if i + 1 < offs.len() {
                bs + offs[i + 1] as usize
            } else {
                be
            }
        };
        let mut streams = Vec::with_capacity(descs.len());
        for (i, d) in descs.iter().enumerate() {
            let (s, en) = (bs + d.off as usize, span_end(i));
            let is_bitmap = matches!(d.code, 0x01 | 0x02 | 0x03); // existence sub-streams only
            let count = (d.param as usize)
                .min((en - s).min(huge_len(en)) * 32)
                .min(64_000_000);
            let (values, bits) = if is_bitmap {
                (vec![], bitfield(b, d.code, s, en, count))
            } else {
                (decode_u32(b, d.code, s, en, count), vec![])
            };
            streams.push(GenAttrStream {
                col: d.kind,
                flags: d.flags,
                code: d.code,
                param: d.param,
                values,
                bits,
            });
        }
        Ok(GenAttrBlock {
            elem_start: e.elem_start,
            elem_end: e.elem_end,
            block_off: e.block_off,
            streams,
        })
    }
}

#[inline]
fn huge_len(en: usize) -> usize {
    en.saturating_sub(0) / 8 + 1
}

struct Desc {
    kind: u32,  // kind & 0xfff
    flags: u32, // kind & 0xf000
    code: u32,
    off: u32,
    param: u32,
}

pub fn is_name_list(b: &[u8]) -> bool {
    // ASF name-list header: u32@0x10 = header size (a smallish sane value),
    // sub-header begins with a plausible element count. A GenAttr file is a *different*
    // container (`NLGenAttrFile`); never treat it as a name-list (that produced garbage).
    if b.len() < 0x100 {
        return false;
    }
    if is_gen_attr(b) {
        return false;
    }
    let hdr = u32(b, 0x10);
    let extra = u32(b, 0x14);
    hdr > 0x26 && hdr < 0x1000 && extra < 0x10_0000 && u32(b, hdr as usize) > 0
}

pub fn read(b_in: &[u8]) -> Result<NameList, String> {
    let b = b_in;
    let n = b.len();
    let hdr = u32(b, 0x10) as usize;
    if hdr + 60 > n {
        return Err("truncated ASF header".into());
    }
    let elem_count = u32(b, hdr);
    let block_count = u16(b, hdr + 4) as usize; // section/vector count of block table
                                                // 7 sections at hdr+24: (u8 code, u32 file-absolute offset)
    let mut sec = [(0u8, 0u32); 7];
    for i in 0..7 {
        let p = hdr + 24 + i * 5;
        if p + 5 > n {
            break;
        }
        sec[i] = (b[p], u32(b, p + 1));
    }
    // section 1 (raw u32) holds block file-offsets; count = block_count.
    let boff_base = sec[1].1 as usize;
    let mut block_off = Vec::with_capacity(block_count);
    for i in 0..block_count {
        let o = boff_base + i * 4;
        if o + 4 > n {
            break;
        }
        block_off.push(u32(b, o) as usize);
    }
    if block_off.is_empty() {
        return Err("no block table".into());
    }

    let mut elements: Vec<Element> = Vec::new();

    for bi in 0..block_off.len() {
        let bs = block_off[bi];
        let be = if bi + 1 < block_off.len() {
            block_off[bi + 1]
        } else {
            n
        };
        if bs + 8 > n {
            break;
        }
        let node_count = u16(b, bs) as usize;
        let _f2 = u16(b, bs + 2);
        let elem_count_b = u16(b, bs + 4) as usize;
        let num_desc = u16(b, bs + 6) as usize;
        if node_count == 0 {
            continue;
        }

        let mut descs: Vec<Desc> = Vec::with_capacity(num_desc);
        let mut p = bs + 8;
        for _ in 0..num_desc {
            if p + 12 > be {
                break;
            }
            let k = u16(b, p);
            descs.push(Desc {
                kind: k & 0xfff,
                flags: k & 0xf000,
                code: u16(b, p + 2),
                off: u32(b, p + 4),
                param: u32(b, p + 8),
            });
            p += 12;
        }
        // Data span of a descriptor row = [off[i], off[i+1]) in DESCRIPTOR-ROW order; the last row runs
        // to the block end. Rows sharing an `off` make the *earlier* ones zero-length alias views — this
        // is the device's streaming rule (same one the byte-exact rebuild oracle uses, section 12.2);
        // the earlier offset-based "next larger off" rule mis-attributed a column's bytes to its
        // neighbours whenever several zero-length rows tied (visible garbage on sections of LID20000).
        let span_end = |d: &Desc| -> usize {
            let row = descs
                .iter()
                .position(|x| x.kind == d.kind && x.flags == d.flags && x.off == d.off)
                .unwrap_or(0);
            if row + 1 < descs.len() {
                (bs + descs[row + 1].off as usize).max(bs + d.off as usize)
            } else {
                be
            }
        };
        // helpers to fetch column by (kind,flags)
        let get = |kind: u32, flags: u32| -> Option<&Desc> {
            descs.iter().find(|d| d.kind == kind && d.flags == flags)
        };

        // out-degree column 0x401 (u16)
        let od = match get(0x401, 0) {
            Some(d) => {
                let mut v = decode_u16(
                    b,
                    d.code,
                    bs + d.off as usize,
                    span_end(d),
                    d.param as usize,
                );
                v.resize(node_count, 0);
                v
            }
            None => continue,
        };
        // childStart[n] / firstEdge via faithful CalculateFirstEdgeIndex + ProcessNode (DFS preorder):
        //   firstEdge[n]=edgeCounter; childStart[n]=base+edgeCounter; edgeCounter+=outDegree[n]; recurse children.
        // children(n) = [childStart[n] .. +outDegree[n]). Iterative to avoid native stack overflow.
        let mut cs = vec![0usize; node_count];
        {
            let mut ec = 0usize;
            let mut node = 0usize;
            let mut base = 1usize;
            while node < node_count {
                let mut stack = vec![node];
                let mut consumed = 0usize;
                while let Some(n) = stack.pop() {
                    cs[n] = base + ec;
                    ec += od[n] as usize;
                    consumed += 1;
                    let c0 = cs[n];
                    for i in (0..od[n] as usize).rev() {
                        let c = c0 + i;
                        if c < node_count {
                            stack.push(c);
                        }
                    }
                }
                node += consumed;
                base += 1;
            }
        }

        // block-link bitmap over NODES (col 0x402, NLBlockLinkAttrVector::HasBlockLink = bit[node]).
        // The bitmap is the 0x4000 sub-stream (sparse set-bit); a leaf with a link is not a terminal here.
        let blocklink: Vec<bool> = match get(0x402, 0x4000).or_else(|| get(0x402, 0)) {
            Some(d) => {
                let nn = (d.param as usize).min(node_count.max(1));
                let mut v = bitfield(b, d.code, bs + d.off as usize, span_end(d), nn);
                v.resize(node_count, false);
                v
            }
            None => vec![false; node_count],
        };

        // edge labels: col 0x403 = two sub-streams. NLEdgeLabelList::Decode: the RAW sub-stream
        // (code 0x11) is the name blob (param = byte length); the other sub-stream is the per-edge
        // byte-offset list (count = node_count-1, absolute offsets into the blob).
        let mut blob: &[u8] = &[];
        let mut loff: Vec<u32> = Vec::new();
        let c403: Vec<&Desc> = descs.iter().filter(|d| d.kind == 0x403).collect();
        if let Some(bd) = c403.iter().copied().find(|d| d.code == 0x11) {
            let bs0 = bs + bd.off as usize;
            blob = &b[bs0.min(n)..(bs0 + bd.param as usize).min(n)];
            if let Some(offd) = c403.iter().copied().find(|d| d.code != 0x11) {
                loff = decode_u32(
                    b,
                    offd.code,
                    bs + offd.off as usize,
                    span_end(offd),
                    offd.param as usize,
                );
            }
        } else if let Some(bd) = c403.first() {
            let bs0 = bs + bd.off as usize;
            blob = &b[bs0.min(n)..(bs0 + bd.param as usize).min(n)];
        }
        let label = |node: usize| -> &[u8] {
            if node == 0 {
                return &blob[..0];
            }
            let a = if node - 1 < loff.len() {
                loff[node - 1] as usize
            } else {
                blob.len()
            };
            let bb = if node < loff.len() {
                loff[node] as usize
            } else {
                blob.len()
            };
            let a = a.min(blob.len());
            let bb = bb.max(a).min(blob.len());
            &blob[a..bb]
        };

        // positions: col 0x407 -> bitmap over element_count (flags 0x4000) + COMPRESSED coords
        // (flags 0): NLPositionAttrVector::Decode stores coords = 2*popcount(bitmap) u32 (X,Y pairs);
        // element i with bit set maps to coords[2*rank], coords[2*rank+1], rank = set-bits before i.
        let has_pos = match get(0x407, 0x4000) {
            Some(d) => bitfield(b, d.code, bs + d.off as usize, span_end(d), elem_count_b),
            None => vec![true; elem_count_b],
        };
        let npos = has_pos.iter().filter(|&&x| x).count() * 2;
        let coords = match get(0x407, 0) {
            Some(d) => decode_u32(b, d.code, bs + d.off as usize, span_end(d), npos),
            None => vec![],
        };
        // belonging-name (city): col 0x40c -> bitmap (flags0) + COMPRESSED values (flags 0x4000)
        let belongs_flag = match get(0x40c, 0) {
            Some(d) => bitfield(b, d.code, bs + d.off as usize, span_end(d), elem_count_b),
            None => vec![false; elem_count_b],
        };
        let nbel = belongs_flag.iter().filter(|&&x| x).count();
        let belongs_vals = match get(0x40c, 0x4000) {
            Some(d) => decode_u32(b, d.code, bs + d.off as usize, span_end(d), nbel),
            None => vec![],
        };

        // Name accumulation over the trie: node's name = parent's name + its incoming-edge label.
        // Iterate every node as a potential root (forest-safe); children(n) = [childStart[n] ..).
        let mut name_of: Vec<Vec<u8>> = vec![Vec::new(); node_count];
        let mut seen = vec![false; node_count];
        let mut stack: Vec<usize> = Vec::new();
        for r in 0..node_count {
            if seen[r] {
                continue;
            }
            seen[r] = true;
            stack.push(r);
            while let Some(node) = stack.pop() {
                let base = cs[node];
                for i in 0..od[node] as usize {
                    let c = base + i;
                    if c >= node_count || seen[c] {
                        continue;
                    }
                    seen[c] = true;
                    let mut s = name_of[node].clone();
                    s.extend_from_slice(label(c));
                    name_of[c] = s;
                    stack.push(c);
                }
            }
        }

        // Element order = terminating-element DFS (CalculateTerminatingElementIndex /
        // ProcessSubTreeTEIC): leaf nodes (outDegree==0, no block-link) assigned in the order
        // leaf-children-then-recurse. element_index = rank in this order; property vectors index by it.
        let mut term: Vec<usize> = Vec::with_capacity(elem_count_b);
        collect_terms(&od, &cs, &blocklink, node_count, &mut term);

        let mut pos_rank = 0usize;
        let mut bel_rank = 0usize;
        for (ei, node) in term.iter().enumerate() {
            let name = decode_name(&name_of[*node]);
            let hp = has_pos.get(ei).copied().unwrap_or(false);
            let (x, y) = if hp {
                let k = pos_rank * 2;
                pos_rank += 1;
                if k + 1 < coords.len() {
                    (coords[k] as i32, coords[k + 1] as i32)
                } else {
                    (0, 0)
                }
            } else {
                (0, 0)
            };
            let belonging = if belongs_flag.get(ei).copied().unwrap_or(false) {
                let v = belongs_vals.get(bel_rank).copied().unwrap_or(0xffff_ffff);
                bel_rank += 1;
                v
            } else {
                0xffff_ffff
            };
            if name.trim().is_empty() {
                continue;
            }
            elements.push(Element {
                category: 0,
                name,
                x_pau: x,
                y_pau: y,
                has_pos: hp,
                belonging,
            });
        }
    }

    // tNLHPosition validity: sub-header u32@+0x0c bit 0x0008_0000 (0x080013/0x080001 on stock POL files);
    // the -1/-1 pair = "file carries no positions" (LID20000 has the flag set but stores -1/-1).
    let origin = if (u32(b, hdr + 0x0c) & 0x0008_0000) != 0 {
        let (x, y) = (u32(b, hdr + 0x10) as i32, u32(b, hdr + 0x14) as i32);
        if x == -1 || y == -1 {
            None
        } else {
            Some((x, y))
        }
    } else {
        None
    };
    Ok(NameList {
        element_count: elem_count,
        block_count: block_off.len(),
        elements,
        origin,
    })
}

/// Element order exactly mirroring `NLAsfBlock::CalculateTerminatingElementIndex` +
/// `ProcessSubTreeTEIC`: a terminating element is a leaf node (`outDegree==0`) with no block-link,
/// assigned DFS — for each node, first its leaf children (in child order), then recurse into its
/// internal children (in child order). Implemented with an explicit stack (deep tries overflow the
/// native call stack otherwise).
fn collect_terms(
    od: &[u32],
    cs: &[usize],
    blocklink: &[bool],
    node_count: usize,
    out: &mut Vec<usize>,
) {
    let mut done = vec![false; node_count];
    let mut stack: Vec<usize> = Vec::new();
    let mut start = 0usize;
    while start < node_count {
        if od[start] != 0 && !done[start] {
            stack.push(start);
            while let Some(n) = stack.pop() {
                if done[n] {
                    continue;
                }
                done[n] = true;
                let deg = od[n] as usize;
                let base = cs[n];
                for i in 0..deg {
                    let c = base + i;
                    if c < node_count && od[c] == 0 && !(c < blocklink.len() && blocklink[c]) {
                        out.push(c);
                    }
                }
                for i in (0..deg).rev() {
                    let c = base + i;
                    if c < node_count && od[c] != 0 && !done[c] {
                        stack.push(c);
                    }
                }
            }
        }
        start += 1;
    }
}

/// Decode a name. The name-list stores each name as one or more **variants separated by
/// 0x09 (tab)**: typically `<ASCII-folded> \t <proper-UTF8 (with diacritics)>`. Return the
/// canonical variant (the one carrying non-ASCII/diacritics, else the longest), so the display
/// name is the diacritic-correct form.
fn decode_name(raw: &[u8]) -> String {
    let segs: Vec<String> = raw
        .split(|&c| c == 0x09 || c == 0x00)
        .filter(|s| !s.is_empty())
        .map(|s| match std::str::from_utf8(s) {
            Ok(t) => t.trim().to_string(),
            Err(_) => s
                .iter()
                .map(|&c| c as char)
                .collect::<String>()
                .trim()
                .to_string(),
        })
        .collect();
    if segs.len() <= 1 {
        return segs.into_iter().next().unwrap_or_default();
    }
    // prefer a segment with diacritics; tie-break by length.
    segs.into_iter()
        .max_by_key(|s| (s.chars().any(|c| (c as u32) > 0x7f), s.chars().count()))
        .unwrap_or_default()
}

// --------------------------------------------------------------------------- writer (encoder)

/// One name-list entry for the writer: a display-name label, its **absolute** position (PAU) and the
/// position of the **owning city** (`city`). Positions are stored the way the device expects (§12.5):
/// entries are grouped into blocks **by city**, and inside a block each coordinate is written as a
/// `position − city` delta — the device adds the *queried city's* position back at load time. `city:
/// None` (e.g. the settlement gazetteer `LID20000`, which stock keeps coordinate-free) writes no positions
/// and a `-1/-1` file origin. Names must not contain byte `0x00`.
#[derive(Debug, Clone)]
pub struct NameEntry {
    pub label: String,
    pub x_pau: i32,
    pub y_pau: i32,
    pub city: Option<(i32, i32)>,
}

struct TrieNode {
    child: Vec<(u8, usize)>, // (edge byte, child handle), insertion order
}

/// Max trie nodes per block. `node_count`/`element_count` are `u16` in the block header, and
/// stock keeps ~11k nodes/block, so we split entries into blocks under this budget.
const MAX_NODES_PER_BLOCK: usize = 10_000;

/// Encode name entries into an ASF `LID*` name-list file (a plain trie, possibly multi-block),
/// the exact byte format `read()` parses back. This is the round-trip oracle for the converter:
/// `read(&encode(&entries)).elements` reproduces the `(name, position)` multiset.
///
/// Position handling follows the device (§12.5): entries that carry an owning `city` are grouped into
/// blocks **per city** and stored as `position − city` deltas (the device adds the queried city back);
/// the file header then advertises the region's SW corner as its file-level anchor (flags `0x0008_0001`).
/// Entries without a city (the settlement gazetteer) are stored without any coordinate stream (flags
/// `0x0008_0000`, origin `-1/-1`) — exactly like the stock `LID20000`.
pub fn encode(entries: &[NameEntry]) -> Vec<u8> {
    let with_city = entries.iter().any(|e| e.city.is_some());

    // --- group by city (first-seen order), then chunk each city under MAX_NODES_PER_BLOCK ---
    let mut groups: Vec<(Option<(i32, i32)>, Vec<&NameEntry>)> = Vec::new();
    for e in entries {
        if let Some(g) = groups.iter_mut().find(|(c, _)| *c == e.city) {
            g.1.push(e);
        } else {
            groups.push((e.city, vec![e]));
        }
    }
    let mut chunks: Vec<(Option<(i32, i32)>, Vec<NameEntry>)> = Vec::new(); // (anchor, entries)
    for (anchor, ge) in groups {
        let mut start = 0usize;
        while start < ge.len() {
            let (mut i, mut est) = (start, 1usize);
            while i < ge.len() {
                let add = ge[i].label.len() + 1; // upper bound (no prefix sharing)
                if i > start && est + add > MAX_NODES_PER_BLOCK {
                    break;
                }
                est += add;
                i += 1;
            }
            chunks.push((
                anchor,
                ge[start..i]
                    .iter()
                    .map(|e| (*e).clone())
                    .collect::<Vec<NameEntry>>(),
            ));
            start = i;
        }
    }

    let blocks: Vec<(Vec<u8>, usize)> = chunks
        .iter()
        .map(|(anchor, c)| build_block(*anchor, c))
        .collect();
    let total_elem: usize = blocks.iter().map(|(_, e)| *e).sum();

    // --- file-level anchor: region SW corner (stock convention) / -1,-1 when no positions ---
    let corner: (i32, i32) = if with_city {
        (
            entries.iter().map(|e| e.x_pau).min().unwrap_or(0),
            entries.iter().map(|e| e.y_pau).min().unwrap_or(0),
        )
    } else {
        (-1, -1)
    };
    let flags: u32 = if with_city { 0x0008_0001 } else { 0x0008_0000 };

    // --- container: header @0x40, then block table (u32 block starts), then blocks ---
    let hdr = 0x40u32;
    let table_at = hdr as usize + 59;
    let table_len = blocks.len() * 4;
    let first_block_at = table_at + table_len;
    let mut f = vec![0u8; hdr as usize];
    f[0..4].copy_from_slice(b"NLID");
    f[0x10..0x14].copy_from_slice(&hdr.to_le_bytes());
    f[0x14..0x18].copy_from_slice(&0u32.to_le_bytes()); // extra
    let mut sh: Vec<u8> = Vec::new();
    sh = put_u32(sh, total_elem as u32); // element_count (global)
    for i in 0..6 {
        sh = put_u16(sh, if i == 0 { blocks.len() as u16 } else { 0 })
    } // [0]=block count
      // overwrite the two trailing u16 of that run (sub-header u32 @+0x0c) with the position flags
    let fl = sh.len() - 4;
    sh[fl..fl + 4].copy_from_slice(&flags.to_le_bytes());
    sh = put_u32(sh, corner.0 as u32);
    sh = put_u32(sh, corner.1 as u32); // file-level tNLHPosition (PAU)
    for i in 0..7 {
        let code = if i == 1 { 0x11u8 } else { 0u8 };
        let off = if i == 1 { table_at as u32 } else { 0u32 };
        sh.push(code);
        sh = put_u32(sh, off);
    }
    assert_eq!(sh.len(), 59);
    f.extend_from_slice(&sh);
    // block-offset table (absolute file offsets)
    let mut offs: Vec<u32> = Vec::with_capacity(blocks.len());
    let mut pos = first_block_at;
    for (blk, _) in &blocks {
        offs.push(pos as u32);
        pos += blk.len();
    }
    for &o in &offs {
        f = put_u32(f, o);
    }
    for (blk, _) in &blocks {
        f.extend_from_slice(blk);
    }
    f
}

/// Build one block (a plain-trie name-list) for a chunk of entries. `anchor` = the city position the
/// stored coordinates are relative to (`None` = no coordinates at all, gazetteer flavor). Returns
/// (bytes, element_count).
fn build_block(anchor: Option<(i32, i32)>, entries: &[NameEntry]) -> (Vec<u8>, usize) {
    // --- build trie (byte edges; every name terminated with 0x00 so it is a leaf) ---
    let mut nodes: Vec<TrieNode> = vec![TrieNode { child: Vec::new() }];
    let mut leaf_of = vec![0usize; entries.len()];
    for (ei, e) in entries.iter().enumerate() {
        let mut h = 0usize;
        for &b in e.label.as_bytes().iter().chain(std::iter::once(&0u8)) {
            let mut nxt = None;
            for &(eb, ch) in nodes[h].child.iter() {
                if eb == b {
                    nxt = Some(ch);
                    break;
                }
            }
            let ch = match nxt {
                Some(c) => c,
                None => {
                    let c = nodes.len();
                    nodes.push(TrieNode { child: Vec::new() });
                    nodes[h].child.push((b, c));
                    c
                }
            };
            h = ch;
        }
        leaf_of[ei] = h;
    }
    let nc = nodes.len();

    // --- number nodes in the DFS-preorder layout ProcessNode/CalculateFirstEdgeIndex expects ---
    let mut outdeg = vec![0u16; nc];
    let mut cs = vec![0usize; nc]; // childStart per output-id
    let mut label_id = vec![Vec::<u8>::new(); nc]; // incoming-edge label per output-id
    let mut id_of = vec![usize::MAX; nc]; // handle -> output id
    let mut id_to_leaf = vec![usize::MAX; nc]; // output id -> entry index (leaf only)
    let mut ec = 0usize;
    {
        let mut stack = vec![0usize]; // handles
        id_of[0] = 0;
        while let Some(h) = stack.pop() {
            let out = id_of[h];
            let deg = nodes[h].child.len();
            outdeg[out] = deg as u16;
            let cstart = 1 + ec;
            cs[out] = cstart;
            for (i, &(b, child)) in nodes[h].child.iter().enumerate() {
                let cid = cstart + i;
                id_of[child] = cid;
                label_id[cid] = vec![b];
            }
            ec += deg;
            for &(_, child) in nodes[h].child.iter().rev() {
                stack.push(child);
            }
        }
        for (ei, &lh) in leaf_of.iter().enumerate() {
            id_to_leaf[id_of[lh]] = ei;
        }
    }

    // --- element order (terminating-leaf DFS) so coords index by element_index ---
    let no_block = vec![false; nc];
    let mut term: Vec<usize> = Vec::with_capacity(entries.len());
    collect_terms(
        &outdeg.iter().map(|&x| x as u32).collect::<Vec<u32>>(),
        &cs,
        &no_block,
        nc,
        &mut term,
    );
    let elem_count = term.len();

    // --- edge-label blob + absolute offsets (code 0x14 VLE) ---
    let mut blob: Vec<u8> = Vec::new();
    let mut loff: Vec<u32> = Vec::with_capacity(nc.saturating_sub(1));
    for id in 1..nc {
        loff.push(blob.len() as u32);
        blob.extend_from_slice(&label_id[id]);
    }

    // --- coordinates in element order: raw u32 interleaved `position - city` (stock flavor §12.5), or
    //     the gazetteer flavor: no coordinate stream at all (stock LID20000 = empty 0x407 streams) ---
    let mut coords: Vec<u8> = Vec::with_capacity(if anchor.is_some() { elem_count * 8 } else { 0 });
    if anchor.is_some() {
        for &id in &term {
            let ei = id_to_leaf[id];
            let (x, y) = if ei != usize::MAX {
                let a = anchor.unwrap();
                (
                    entries[ei].x_pau as i32 - a.0,
                    entries[ei].y_pau as i32 - a.1,
                )
            } else {
                (0, 0)
            };
            coords = put_u32(coords, x as u32);
            coords = put_u32(coords, y as u32);
        }
    }

    // --- numeric/flag streams ---
    let mut od_bytes: Vec<u8> = Vec::with_capacity(nc * 2);
    for &d in &outdeg {
        od_bytes = put_u16(od_bytes, d)
    }
    let loff_bytes: Vec<u8> = loff.iter().fold(Vec::new(), |acc, &o| {
        let mut a = acc;
        a.extend_from_slice(&vle_encode(o));
        a
    });
    let bl_bits = vec![0u8; (nc + 7) / 8]; // block-link bitmap: none
    let bel_bits = vec![0u8; (elem_count + 7) / 8]; // belonging: none

    // --- assemble block: 8B header + descriptors + data (offsets block-relative) ---
    // Stock position flavor (§12.5): the existence row is a *zero-length* bitmap sub-stream with count
    // `elem_count` — code 0x03 (sparse-clear) reads as "every element has a position", 0x02 (sparse-set)
    // as "none do"; either way its `off` equals the coordinate row's `off`.
    let (pos_wf, pos_code, pos_bytes): (u16, u16, &[u8]) = if anchor.is_some() {
        (0x4407, 0x03, &[] as &[u8])
    } else {
        (0x4407, 0x02, &[] as &[u8])
    };
    let (coord_code, coord_param): (u16, u32) = if anchor.is_some() {
        (0x11, (elem_count * 2) as u32)
    } else {
        (0x11, 0)
    };
    let descs: Vec<(u16, u16, &[u8], u32)> = vec![
        (0x0401, 0x11, &od_bytes, nc as u32),     // outDegree (raw u16)
        (0x4402, 0x01, &bl_bits, nc as u32),      // block-link bitmap (raw bits, flags 0x4000)
        (0x0403, 0x11, &blob, blob.len() as u32), // edge-label blob (raw bytes)
        (0x4403, 0x14, &loff_bytes, loff.len() as u32), // edge-label offsets (VLE, flags 0x4000)
        (pos_wf, pos_code, pos_bytes, elem_count as u32), // has-position sub-stream (flags 0x4000, tie off)
        (0x0407, coord_code, &coords, coord_param), // coords interleaved X,Y (raw u32) or empty
        (0x040c, 0x01, &bel_bits, elem_count as u32), // belonging bitmap
    ];

    let hdr_sz = 8usize + descs.len() * 12;
    let mut block: Vec<u8> = Vec::new();
    block = put_u16(block, nc as u16);
    block = put_u16(block, 0);
    block = put_u16(block, elem_count as u16);
    block = put_u16(block, descs.len() as u16);
    let mut cur = hdr_sz;
    let mut data: Vec<u8> = Vec::new();
    for (kwf, code, bytes, param) in &descs {
        block = put_u16(block, *kwf);
        block = put_u16(block, *code);
        block = put_u32(block, cur as u32);
        block = put_u32(block, *param);
        data.extend_from_slice(bytes);
        cur += bytes.len();
    }
    block.extend_from_slice(&data);
    (block, elem_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic `NLGenAttrFile`: 0x26 outer header (`hdr_size`@0x10), then the sub-header
    /// `{elem_count, x, toc_count, block_count}` at `hdr`, then `toc_count` 12-byte TOC entries
    /// tiling `[0, elem_count)`. This is the layout that `read` previously mis-parsed as a
    /// name-list section table (returning garbage); `read_gen_attr` must parse it instead.
    fn synth_gen_attr(elem_count: u32, per_block: u32) -> Vec<u8> {
        let hdr = 0x77usize;
        let mut b = vec![0u8; hdr];
        b[0] = 0x02;
        b[1] = 0x04; // region
        b[0x10..0x14].copy_from_slice(&(hdr as u32).to_le_bytes());
        let toccount = elem_count.div_ceil(per_block);
        b.extend_from_slice(&elem_count.to_le_bytes());
        b.extend_from_slice(&0u32.to_le_bytes()); // this+4
        b.extend_from_slice(&toccount.to_le_bytes());
        b.extend_from_slice(&(toccount * 13).to_le_bytes()); // this+12 (some other count)
        let mut off = 0x1000u32;
        for i in 0..toccount {
            let a = i * per_block;
            let c = ((i + 1) * per_block - 1).min(elem_count - 1);
            b.extend_from_slice(&a.to_le_bytes());
            b.extend_from_slice(&c.to_le_bytes());
            b.extend_from_slice(&off.to_le_bytes());
            off += 0x1000;
        }
        b.resize(0x400, 0x00); // real GenAttr files are MB-sized; pad past the `is_gen_attr` size floor
        b
    }

    #[test]
    fn gen_attr_toc_parses_and_tiles() {
        let b = synth_gen_attr(20000, 7904);
        assert!(is_gen_attr(&b));
        assert!(
            !is_name_list(&b),
            "GenAttr must not be mistaken for a name-list"
        );
        let ga = read_gen_attr(&b).unwrap();
        assert_eq!(ga.element_count, 20000);
        assert_eq!(ga.blocks[0].elem_start, 0);
        assert_eq!(ga.blocks.last().unwrap().elem_end, 19999);
        assert!(ga
            .blocks
            .windows(2)
            .all(|w| w[0].elem_end + 1 == w[1].elem_start));
    }

    #[test]
    fn gen_attr_rejects_bad_toc() {
        // a broken tiling (a gap) must not validate
        let mut b = synth_gen_attr(10000, 5000);
        // corrupt the 2nd entry's elem_start (offset hdr+16+12+0) to introduce a gap
        let p = 0x77 + 16 + 12;
        b[p..p + 4].copy_from_slice(&6000u32.to_le_bytes()); // should have been 5000
        assert!(!is_gen_attr(&b));
        assert!(read_gen_attr(&b).is_err());
    }

    #[test]
    fn gen_attr_block_decodes_descriptors() {
        // one block with a raw-bitmap descriptor (0xc09) + a raw-u32 descriptor (0xc01), laid down
        // block-relative; decode_block must parse the 12-byte descriptor table and run the codecs.
        let mut b = synth_gen_attr(16, 8); // toccount=2 (>=2 required), block_off[0]=0x1000
        let bs = 0x1000usize;
        b.resize(bs + 0x40, 0xAA);
        b[bs..bs + 2].copy_from_slice(&2u16.to_le_bytes()); // num_desc
                                                            // desc0: kind 0x0c09 (col 0xc09, flags 0), code 0x01 raw bitmap, off 0x20, param 8 bits
        b[bs + 2..bs + 4].copy_from_slice(&0x0c09u16.to_le_bytes());
        b[bs + 4..bs + 6].copy_from_slice(&0x0001u16.to_le_bytes());
        b[bs + 6..bs + 10].copy_from_slice(&0x20u32.to_le_bytes());
        b[bs + 10..bs + 14].copy_from_slice(&8u32.to_le_bytes());
        // desc1: kind 0x0c01 (col 0xc01, flags 0), code 0x11 raw u32, off 0x24, param 4 values
        b[bs + 14..bs + 16].copy_from_slice(&0x0c01u16.to_le_bytes());
        b[bs + 16..bs + 18].copy_from_slice(&0x0011u16.to_le_bytes());
        b[bs + 18..bs + 22].copy_from_slice(&0x24u32.to_le_bytes());
        b[bs + 22..bs + 26].copy_from_slice(&4u32.to_le_bytes());
        // data: bitmap byte at bs+0x20 (bits 0,2,5 set = 0b0010_0101), 4 u32 at bs+0x24
        b[bs + 0x20] = 0b0010_0101;
        for (i, v) in [10u32, 20, 30, 40].iter().enumerate() {
            let o = bs + 0x24 + i * 4;
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        let ga = read_gen_attr(&b).unwrap();
        let blk = ga.decode_block(&b, 0).unwrap();
        assert_eq!(blk.streams.len(), 2);
        let bm = blk.streams.iter().find(|s| s.col == 0xc09).unwrap();
        assert_eq!(bm.bits.len(), 8);
        assert_eq!(
            bm.bits,
            vec![true, false, true, false, false, true, false, false]
        );
        let vs = blk.streams.iter().find(|s| s.col == 0xc01).unwrap();
        assert_eq!(vs.values, vec![10, 20, 30, 40]);
    }

    #[test]
    #[ignore] // needs the reference card mounted
    fn gen_attr_stock_block0_decodes() {
        let path = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/LID40006.DAT";
        let b = std::fs::read(path).expect("reference card LID40006.DAT");
        let ga = read_gen_attr(&b).unwrap();
        let blk = ga.decode_block(&b, 0).unwrap();
        assert_eq!(ga.element_count, 974871);
        assert_eq!(blk.elem_start, 0);
        assert_eq!(blk.elem_end, 7903);
        // parity bitmap col 0xc09 must decode to param bits with a plausible ~half density
        let par = blk.streams.iter().find(|s| s.col == 0xc09).unwrap();
        assert_eq!(par.bits.len(), 10886);
        let set = par.bits.iter().filter(|&&x| x).count();
        assert!(
            set > 4000 && set < 8000,
            "parity density implausible: {set}"
        );
    }

    #[test]
    #[ignore] // needs the reference card mounted
    fn gen_attr_stock_rebuild_byte_exact() {
        let path = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/LID40006.DAT";
        let b = std::fs::read(path).expect("reference card LID40006.DAT");
        let nblocks = read_gen_attr(&b).unwrap().blocks.len();
        let (ok, mm) = rebuild::rebuild_all(&b);
        if let Some(m) = mm {
            panic!(
                "block {}/{} row {}: kind={:#04x} code={:#03x} param={} off={:#x} regpos={:#x} :: {}",
                m.block, nblocks, m.row, m.kind, m.code, m.param, m.off, m.regpos, m.detail
            );
        }
        assert_eq!(ok, nblocks, "all blocks must byte-rebuild exactly");
    }

    /// Synthetic GenAttr HnR block: streets -> addr lists, addr -> number range, addr -> owner street-desc,
    /// parity. Write -> `read_gen_attr`/`decode_block` -> replay the car's member lookups and assert the
    /// answers match what we encoded. Proves `write` is byte-valid *and* read-model-consistent offline
    /// (the LID2 element-id join itself is external — these use block-internal street-desc indices).
    #[test]
    fn gen_attr_writer_roundtrip_device_model() {
        use crate::write::{write_gen_attr_file, BlockData, ColData};
        // block0: elems 0..1 (addrs 0,1); street-list = 2 streets -> addrs {0,1}/{2,3-per-blk0:false}.
        let mk = |e0: u32, e1: u32, addr0: u32, addr1: u32| BlockData {
            elem_start: e0,
            elem_end: e1,
            cols: vec![
                // +0x28 (0xc01) street -> addr list. street0 owns both addrs, street1 none.
                ColData {
                    selector: 0xc01,
                    domain: 2,
                    exists: vec![true, false],
                    counts: vec![2],
                    values: vec![addr0, addr1],
                    code_8000: 0x16,
                    code_0000: 0x14,
                    range_from_to: None,
                },
                // +0x4d0 (0xc13) owner-descr -> street-desc list (addr idx = owner-descr, 1:1).
                ColData {
                    selector: 0xc13,
                    domain: 2,
                    exists: vec![true, true],
                    counts: vec![1, 2],
                    values: vec![0, 1],
                    code_8000: 0x16,
                    code_0000: 0x14,
                    range_from_to: None,
                },
                // +0x300 (0x00c) Range<u32> number per addr.
                ColData {
                    selector: 0x00c,
                    domain: 2,
                    exists: vec![true, true],
                    counts: vec![],
                    values: vec![],
                    code_8000: 0x16,
                    code_0000: 0x11,
                    range_from_to: Some((
                        vec![addr0 * 10, addr1 * 10],
                        vec![addr0 * 10, addr1 * 10],
                    )),
                },
                // +0x280 (0xc0a) tBitArray per-addr parity-even existence.
                ColData {
                    selector: 0xc0a,
                    domain: 2,
                    exists: vec![true, false],
                    counts: vec![],
                    values: vec![],
                    code_8000: 0x16,
                    code_0000: 0x14,
                    range_from_to: None,
                },
            ],
        };
        let file = write_gen_attr_file(4, &[], &[mk(0, 1, 0, 1), mk(2, 3, 2, 3)]);

        let gi = read_gen_attr(&file).expect("self-written file must re-parse");
        assert_eq!(gi.element_count, 4);
        assert_eq!(gi.blocks.len(), 2);
        let (ok, mm) = crate::rebuild::rebuild_all(&file);
        assert!(mm.is_none(), "self-written file must byte-rebuild: {mm:?}");
        assert_eq!(ok, 2);

        for (bi, (a0, a1)) in [(0usize, (0u32, 1u32)), (1, (2, 3))] {
            let blk = gi.decode_block(&file, bi).unwrap();
            let st = |col: u32, flag: u32| {
                blk.streams
                    .iter()
                    .find(|s| s.col == col && s.flags == flag)
                    .unwrap_or_else(|| panic!("blk {bi} missing col {col:03x}/{flag:04x}"))
            };
            // +0x28: street0 -> [addr0, addr1].
            let ex = st(0xc01, 0x4000);
            let off = st(0xc01, 0x8000);
            let val = st(0xc01, 0);
            assert_eq!(ex.bits, vec![true, false]);
            assert_eq!(
                &val.values[..off.values[0] as usize],
                [a0, a1],
                "blk {bi} street0 addr list"
            );
            // +0x4d0: owner-descr(addr idx) -> street-desc.
            let ooff = st(0xc13, 0x8000);
            let oval = st(0xc13, 0);
            assert_eq!(oval.values, vec![0, 1]);
            assert_eq!(&oval.values[..ooff.values[0] as usize], [0]); // addr0 -> street-desc 0
                                                                      // +0x300 Range number.
            assert_eq!(st(0x00c, 0x8000).values, vec![a0 * 10, a1 * 10]);
            assert_eq!(st(0x00c, 0).values, vec![a0 * 10, a1 * 10]);
            // parity existence.
            assert_eq!(st(0xc0a, 0x4000).bits, vec![true, false]);
        }
    }
}
