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

pub mod header;
pub mod pa;
mod rebuild;
pub mod rel;
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

/// Encode values with the device's Simple9 scheme (code `0x18`, §11.4). `bits_max` is 28 for the
/// u32 flavor (`DecodeSimple9<u32>`) and 16 for the u16 flavor (mode 9 narrows to `bits9`).
pub fn simple9_encode(values: &[u32], bits_max: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < values.len() {
        let mut chosen = None;
        for m in 1..=9u32 {
            let (cnt, bits) = match simple9_mode(m) {
                Some((c, b)) => (c, if m == 9 { bits9_of(bits_max) } else { b }),
                None => continue,
            };
            let take = cnt.min(values.len() - i);
            let maxv = values[i..i + take].iter().fold(0u32, |a, &v| a.max(v));
            if (maxv as u64) < (1u64 << bits) {
                chosen = Some((m, cnt, bits, take));
                break;
            }
        }
        let (m, _cnt, bits, take) = chosen.expect("value wider than codec");
        let mut w = 0u32;
        for k in (0..take).rev() {
            w = (w << bits) | (values[i + k] & ((1u32 << bits) - 1));
        }
        w |= m << 28; // mode nibble lands in bits 28..32 AFTER packing the value window
        out.extend_from_slice(&w.to_le_bytes());
        i += take;
    }
    out
}

fn bits9_of(bits_max: usize) -> usize {
    bits_max
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
pub(crate) fn decode_u32(b: &[u8], code: u32, start: usize, end: usize, n: usize) -> Vec<u32> {
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
    pub block: usize, // 0-based NLAsfBlock index this element belongs to
    pub category: u16,
    pub name: String,
    /// Full raw device string of this element's trie path **before** the display cut (line 1 =
    /// ASCII-folded sort/type-in form, then TAB-separated extra lines: settlement number + the
    /// diacritic/original form). `name` is the post-first-TAB display line (empty when line 1 had
    /// no content), per `vCollectNamesOfCat`. Kept latin-1-decoded for invalid UTF-8 fidelity.
    pub sort_name: String,
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
/// `u32 elem_count(=this+0x80), u32 x(=this+0x84 = filesize − first block_off), u32 toc_count(=this
/// +0x88)`, then `toc_count × NLBlockTocEntry` starting **at `hdr+12`** of 3 `u32` each:
/// `{u32 block_off, NLInterval{u32 elem_start, u32 elem_end}}` — exactly the field order
/// `GetBlockDescr` @0xe0dd4c consumes (`*off = entry[0]`, interval = `entry+4`, block byte size =
/// `next.block_off − this.block_off`, last block runs to EOF). Grouping the table as
/// (elem_start, elem_end, block_off) from `hdr+16` — a −4-byte misgroupBy — makes every block read
/// the *next* block's byte window (the historic bug: 0xc01 "domains" appeared shifted, D_N = W_{N+1},
/// and last-entry offsets decoded as garbage). Distinct from `NLNameList::LoadHeader`'s VLE/raw
/// section table.
fn gen_attr_toc(b: &[u8]) -> Option<(u32, Vec<GenAttrTocEntry>)> {
    let n = b.len();
    let hdr = u32(b, 0x10) as usize;
    if hdr + 24 > n {
        return None;
    }
    let elem_count = u32(b, hdr);
    let toccount = u32(b, hdr + 8);
    if toccount < 2 || toccount > 1_000_000 || elem_count < 1 || elem_count > 0x7fff_ffff {
        return None;
    }
    let mut p = hdr + 12;
    let mut blocks: Vec<GenAttrTocEntry> = Vec::with_capacity((toccount as usize).min(n / 12));
    for i in 0..toccount {
        if p + 12 > n {
            break;
        }
        let (o, a, c) = (u32(b, p), u32(b, p + 4), u32(b, p + 8));
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

/// A crossing (`LID3%05u` = fileID+10000) file index. Elements are the crossings themselves and
/// live in the *street list's* element-id domain (DEU `LID30006` `elem_count` = the
/// `LID20006` element count). Header kind `2`; sub-header (`NLCrossingFile::DecodeSubHeader`
/// @0xe08eb0) = `{u32 elem_count, u32 X, u32 file_size, u32 block_count, TOC[block_count]
/// {u32 block_off, NLInterval{u32 elem_start, u32 elem_end_incl}}}` — same TOC shape as GenAttr,
/// 4 bytes of extra sub-header fields. RE-verified 2026-09 against stock DEU (test
/// `crossing_re.rs`); the LEGACY-variant cards (author @0x0c, `02 77 ?? ??` @0x08 — e.g. the POL
/// card) fail this parse by design: their sub-header slot holds author garbage.
pub struct CrossingIndex {
    pub element_count: u32,
    /// Sub-header `X` (= GenAttr `x`: file_size − first block_off on stock modern-gen DEU).
    pub x: u32,
    pub blocks: Vec<GenAttrTocEntry>,
}

fn crossing_toc(b: &[u8]) -> Option<(u32, u32, Vec<GenAttrTocEntry>)> {
    let n = b.len();
    let hdr = u32(b, 0x10) as usize;
    if hdr + 32 > n || u32(b, 0x0c) != header::KIND_CROSSING {
        return None;
    }
    let (elem_count, x, fsize, toccount) = (
        u32(b, hdr),
        u32(b, hdr + 4),
        u32(b, hdr + 8),
        u32(b, hdr + 12),
    );
    if !(2..=1_000_000).contains(&toccount) || !(1..=0x7fff_ffff).contains(&elem_count) {
        return None;
    }
    let toc_end = hdr.checked_add(16)?.checked_add(12 * toccount as usize)?;
    let mut p = hdr + 16;
    let mut blocks: Vec<GenAttrTocEntry> = Vec::with_capacity((toccount as usize).min(n / 12));
    for i in 0..toccount {
        if p + 12 > n {
            break;
        }
        let (o, a, c) = (u32(b, p), u32(b, p + 4), u32(b, p + 8));
        p += 12;
        let contiguous = i == 0 || a == blocks[i as usize - 1].elem_end + 1;
        let off_ok = (o as usize) >= toc_end
            && (o as usize) < n
            && (i == 0 || o > blocks[i as usize - 1].block_off);
        if !contiguous || a > c || c >= elem_count || !off_ok {
            break;
        }
        let _ = fsize;
        blocks.push(GenAttrTocEntry {
            elem_start: a,
            elem_end: c,
            block_off: o,
        });
    }
    // accept only a full tiling of [0, elem_count) and complete TOC consumption
    if blocks.len() != toccount as usize
        || blocks[0].elem_start != 0
        || blocks.last().unwrap().elem_end != elem_count - 1
    {
        return None;
    }
    Some((elem_count, x, blocks))
}

/// True if `b` is a modern-gen crossing file with a sane first block (descriptor table leading
/// with the existence-bitmap row of column 0x801 — the street-index column).
pub fn is_crossing(b: &[u8]) -> bool {
    match crossing_toc(b) {
        None => false,
        Some((_, _, blocks)) => {
            let bs = blocks[0].block_off as usize;
            if bs + 14 > b.len() {
                return false;
            }
            let num_desc = u16(b, bs) as usize;
            (1..=200).contains(&num_desc) && u16(b, bs + 2) & 0xfff == 0x801
        }
    }
}

/// Parse a crossing file index (modern-gen `LID3%05u` containers only).
pub fn read_crossings(b: &[u8]) -> Result<CrossingIndex, String> {
    crossing_toc(b)
        .map(|(element_count, x, blocks)| CrossingIndex {
            element_count,
            x,
            blocks,
        })
        .ok_or_else(|| "not a modern-gen crossing file (kind != 2 or TOC does not tile)".into())
}

/// One decoded crossing: its element id (street-list id space) and the crossing's street
/// element-ids (indices into the name list read by `read`, via `enDecodeCrossings` member +0x28 /
/// `enGetCrossingStreetIndex 0x00e07af0`).
#[derive(Debug, Clone)]
pub struct Crossing {
    pub element: u32,
    pub streets: Vec<u32>,
}

impl CrossingIndex {
    /// Decode the crossings of block `bi` = the column-0x801 `NLValueListAttrVector`
    /// (existence bitmap 0x4801 / value-starts 0x8801 / values 0x0801; `starts` are absolute
    /// offsets into `values`, the final row's end implicit at `values.len()`). Street-ids resolve
    /// against the street name list of the same partition (`fileID`).
    pub fn decode_crossings(&self, b: &[u8], bi: usize) -> Result<Vec<Crossing>, String> {
        let e = *self.blocks.get(bi).ok_or("block index out of range")?;
        let n = b.len();
        let bs = e.block_off as usize;
        let be = if bi + 1 < self.blocks.len() {
            self.blocks[bi + 1].block_off as usize
        } else {
            n
        };
        if bs + 2 > n {
            return Err("truncated crossing block".into());
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
        let offs: Vec<u32> = descs.iter().map(|d| d.off).collect();
        let span_end = |i: usize| -> usize {
            if i + 1 < offs.len() {
                bs + offs[i + 1] as usize
            } else {
                be
            }
        };
        let mut bits: Vec<bool> = Vec::new();
        let mut starts: Vec<u32> = Vec::new();
        let mut values: Vec<u32> = Vec::new();
        for (i, d) in descs.iter().enumerate() {
            if d.kind != 0x801 || d.flags == 0xc000 {
                continue;
            }
            let (s, en) = (bs + d.off as usize, span_end(i));
            let count = d.param as usize;
            if matches!(d.flags, 0x4000) {
                bits = bitfield(b, d.code, s, en, count);
            } else if matches!(d.flags, 0x8000) {
                starts = decode_u32(b, d.code, s, en, count);
            } else {
                values = decode_u32(b, d.code, s, en, count);
            }
        }
        if values.is_empty() {
            return Ok(Vec::new());
        }
        if starts.is_empty() {
            return Err(format!("block {bi}: 0x801 values without starts"));
        }
        let n_elem = e.elem_end - e.elem_start + 1;
        let mut out = Vec::new();
        let mut j = 0usize;
        for local in 0..n_elem as usize {
            if bits.get(local).copied().unwrap_or(true) {
                let st = starts.get(j).copied().unwrap_or(values.len() as u32) as usize;
                let en = starts
                    .get(j + 1)
                    .map(|x| *x as usize)
                    .unwrap_or_else(|| values.len())
                    .max(st);
                if en > values.len() {
                    return Err(format!("block {bi}: VL range {st}..{en} past values"));
                }
                out.push(Crossing {
                    element: e.elem_start + local as u32,
                    streets: values[st..en].to_vec(),
                });
                j += 1;
            }
        }
        Ok(out)
    }
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
            let count = if matches!(d.code, 0x02 | 0x03) {
                // Sparse bitmaps are self-delimiting: `param` is authoritative, bytes may be *shorter*
                // than worst case or empty (code `0x02` with zero bytes = the stock "all-missing"
                // string-vector existence row; code `0x03` zero bytes = "all present").
                (d.param as usize).min(64_000_000)
            } else {
                (d.param as usize)
                    .min((en - s).min(huge_len(en)) * 32)
                    .min(64_000_000)
            };
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
    if is_crossing(b) {
        return false;
    }
    let hdr = u32(b, 0x10);
    let extra = u32(b, 0x14);
    hdr > 0x26 && hdr < 0x1000 && extra < 0x10_0000 && u32(b, hdr as usize) > 0
}

/// One parsed `NLAsfBlock`: trie tables + per-block element columns, kept raw so the
/// cross-block naming pass can run over the whole namelist afterwards.
struct ParsedBlock<'a> {
    node_count: usize,
    od: Vec<u32>,
    fe: Vec<usize>,
    cs: Vec<usize>,
    blob: &'a [u8],
    loff: Vec<u32>,
    roots: Vec<usize>,
    links: std::collections::HashMap<usize, (usize, usize)>,
    term_nodes: Vec<usize>,
    raw_local: Vec<Vec<u8>>,
    has_pos: Vec<bool>,
    coords: Vec<u32>,
    belongs_flag: Vec<bool>,
    belongs_vals: Vec<u32>,
}

impl<'a> ParsedBlock<'a> {
    fn lab(&self, e: usize) -> &'a [u8] {
        lab_loc(self.blob, &self.loff, e)
    }
}

fn lab_loc<'a>(blob: &'a [u8], loff: &[u32], e: usize) -> &'a [u8] {
    let a = *loff.get(e).unwrap_or(&(blob.len() as u32)) as usize;
    let bb = *loff.get(e + 1).unwrap_or(&(blob.len() as u32)) as usize;
    let a = a.min(blob.len());
    let bb = bb.max(a).min(blob.len());
    &blob[a..bb]
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

    let mut bp: Vec<ParsedBlock> = Vec::new();

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
        // firstEdge/childStart per node, exactly like the device: `CalculateFirstEdgeIndex` runs
        // `ProcessNode` as a PURE DFS with no visited-guard — in the shared-node (DAWG) layout a node
        // is reached through several parents and its stored firstEdge/childStart hold the values from
        // the LAST visit (overwrite per visit). childStart[n] = treeBase + firstEdge[n];
        // children(n) = [childStart[n] .. +outDegree[n]). Root loop jumps by the subtree VISIT count.
        let mut cs = vec![0usize; node_count];
        let mut fe = vec![usize::MAX; node_count];
        let mut roots: Vec<usize> = Vec::new();
        {
            let mut ec = 0usize;
            let mut root = 0usize;
            let mut base = 1usize;
            while root < node_count {
                roots.push(root);
                let mut stack = vec![root];
                let mut visits = 0usize;
                while let Some(n) = stack.pop() {
                    fe[n] = ec;
                    cs[n] = base + ec;
                    ec += od[n] as usize;
                    visits += 1;
                    let c0 = cs[n];
                    for i in (0..od[n] as usize).rev() {
                        let c = c0 + i;
                        if c < node_count {
                            stack.push(c);
                        }
                    }
                }
                root += visits;
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
        // NLBlockLinkAttrVector::Decode (00cde84c): the linked NODES (bitmap above) pair up, in order,
        // with u32 VLE pairs in the flags-0 sub-stream (code 0x14): (target block index, target node
        // index). `NLInputStep::bStepDown` (00cecf78) then RE-HOMES the search to that node in that
        // block while the accumulator string keeps running — the namelist trie is a DAG spanning
        // blocks (`enGetNodeBlockLink` + `poGetContainer`). Names therefore need the cross-block walk.
        let mut links: std::collections::HashMap<usize, (usize, usize)> =
            std::collections::HashMap::new();
        if let (Some(bd), Some(vd)) = (get(0x402, 0x4000), get(0x402, 0)) {
            let nn = (bd.param as usize).min(node_count);
            let bits = bitfield(b, bd.code, bs + bd.off as usize, span_end(bd), nn);
            let ones: Vec<usize> = (0..node_count.min(bits.len()))
                .filter(|&i| bits[i])
                .collect();
            let vals = decode_u32(
                b,
                vd.code,
                bs + vd.off as usize,
                span_end(vd),
                vd.param as usize * 2,
            );
            for (k, &nd) in ones.iter().enumerate() {
                if 2 * k + 1 < vals.len() {
                    links.insert(nd, (vals[2 * k] as usize, vals[2 * k + 1] as usize));
                }
            }
        }

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
        // (edges are consumed by `fe[n]+i` in the element walk below, not by child id)

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
        // belonging-name (city): col 0x40c -> existence bitmap on the 0x4000 sub-stream + values on the
        // flags-0 sub-stream (stock template row order §11.4b: `0x440c` bitmap row, then `0x040c` values).
        let belongs_flag = match get(0x40c, 0x4000).or_else(|| get(0x40c, 0)) {
            Some(d) => bitfield(b, d.code, bs + d.off as usize, span_end(d), elem_count_b),
            None => vec![false; elem_count_b],
        };
        let nbel = belongs_flag.iter().filter(|&&x| x).count();
        let belongs_vals = match get(0x40c, 0) {
            Some(d) if d.code != 0x01 && d.code != 0x02 && d.code != 0x03 => {
                decode_u32(b, d.code, bs + d.off as usize, span_end(d), nbel)
            }
            Some(_) => vec![], // a bitmap-pattern row here means "no values" (empty optional column)
            None => vec![],
        };

        // Element names: replay `CalculateTerminatingElementIndex`/`ProcessSubTreeTEIC` — a PURE DFS
        // per forest root over the DAG (a shared node is visited once per incoming path; its element
        // name is the labels accumulated along THAT path, appended exactly like
        // `NLInputStep::bSetSelectedEdge` does when the device walks). At each node: first register
        // the leaf children (od==0, no block-link) as elements, then recurse into internal children.
        // Raw label per (parent n, i-th child) = blob[loff[fe[n]+i] .. loff[fe[n]+i+1]].
        let (term_nodes, raw_local): (Vec<usize>, Vec<Vec<u8>>) = {
            let mut tnodes: Vec<usize> = Vec::with_capacity(elem_count_b);
            let mut out: Vec<Vec<u8>> = Vec::with_capacity(elem_count_b);
            let mut stack: Vec<(usize, Vec<u8>)> = Vec::new(); // (node, accumulated string so far)
            let mut root = 0usize;
            while root < node_count {
                let mut edgevisits = 0usize; // ProcessSubTreeTEIC return value for this root
                stack.push((root, Vec::new()));
                while let Some((n, s)) = stack.pop() {
                    edgevisits += od[n] as usize;
                    let base = cs[n];
                    let fen = fe[n];
                    for i in 0..od[n] as usize {
                        let c = base + i;
                        if c < node_count && od[c] == 0 && !blocklink[c] {
                            let mut t = s.clone();
                            t.extend_from_slice(lab_loc(&blob, &loff, fen + i));
                            tnodes.push(c);
                            out.push(t);
                        }
                    }
                    for i in (0..od[n] as usize).rev() {
                        let c = base + i;
                        if c < node_count && od[c] != 0 {
                            let mut t = s.clone();
                            t.extend_from_slice(lab_loc(&blob, &loff, fen + i));
                            stack.push((c, t));
                        }
                    }
                }
                root += edgevisits + 1;
            }
            (tnodes, out)
        };

        bp.push(ParsedBlock {
            node_count,
            od,
            fe,
            cs,
            blob,
            loff,
            roots,
            links,
            term_nodes,
            raw_local,
            has_pos,
            coords,
            belongs_flag,
            belongs_vals,
        });
    }

    // Cross-block naming (`NLInputStep::bStepDown` 00cecf78): the block 0 trie is the trunk of the
    // namelist forest; every block-link node re-homes the walk to (target block, target node) with
    // the accumulated string kept running. Blocks are emitted in file order and links always point
    // forward, so one ascending sweep resolves every reached terminal's full raw string. Terminals
    // of subtrees that no link addresses keep their block-local spelling (their string starts at
    // the local root, empty prefix — identical to the old per-block reading).
    let mut global: std::collections::HashMap<(usize, usize), Vec<u8>> =
        std::collections::HashMap::new();
    let mut entries: Vec<Vec<(usize, Vec<u8>)>> = vec![Vec::new(); bp.len()];
    if let Some(first) = bp.first() {
        for &r in &first.roots {
            entries[0].push((r, Vec::new()));
        }
    }
    for bi in 0..bp.len() {
        let blk = &bp[bi];
        let mut stack: Vec<(usize, Vec<u8>)> = std::mem::take(&mut entries[bi]);
        while let Some((node, s)) = stack.pop() {
            if node >= blk.node_count {
                continue;
            }
            let deg = blk.od[node] as usize;
            let base = blk.cs[node];
            let fen = blk.fe[node];
            let link = blk.links.get(&node).copied();
            if let Some((tb, tn)) = link {
                if tb < bp.len() && tb > bi {
                    entries[tb].push((tn, s.clone()));
                }
            }
            if deg == 0 {
                if link.is_none() || !matches!(link, Some((tb, _)) if tb < bp.len() && tb > bi) {
                    global.entry((bi, node)).or_insert_with(|| s.clone());
                }
                continue;
            }
            for i in 0..deg {
                let c = base + i;
                if c >= blk.node_count || blk.od[c] != 0 {
                    continue;
                }
                let mut t = s.clone();
                t.extend_from_slice(blk.lab(fen + i));
                match blk.links.get(&c).copied() {
                    Some((tb, tn)) if tb < bp.len() && tb > bi => entries[tb].push((tn, t)),
                    _ => {
                        global.entry((bi, c)).or_insert_with(|| t);
                    }
                }
            }
            for i in (0..deg).rev() {
                let c = base + i;
                if c < blk.node_count && blk.od[c] != 0 {
                    let mut t = s.clone();
                    t.extend_from_slice(blk.lab(fen + i));
                    stack.push((c, t));
                }
            }
        }
    }

    let mut elements: Vec<Element> = Vec::new();
    for (bi, blk) in bp.iter().enumerate() {
        let mut pos_rank = 0usize;
        let mut bel_rank = 0usize;
        for ei in 0..blk.term_nodes.len() {
            let raw: &[u8] = match global.get(&(bi, blk.term_nodes[ei])) {
                Some(t) => t,
                None => &blk.raw_local[ei],
            };
            let name = decode_name(raw);
            let hp = blk.has_pos.get(ei).copied().unwrap_or(false);
            let (x, y) = if hp {
                let k = pos_rank * 2;
                pos_rank += 1;
                if k + 1 < blk.coords.len() {
                    (blk.coords[k] as i32, blk.coords[k + 1] as i32)
                } else {
                    (0, 0)
                }
            } else {
                (0, 0)
            };
            let belonging = if blk.belongs_flag.get(ei).copied().unwrap_or(false) {
                let v = blk
                    .belongs_vals
                    .get(bel_rank)
                    .copied()
                    .unwrap_or(0xffff_ffff);
                bel_rank += 1;
                v
            } else {
                0xffff_ffff
            };
            if name.trim().is_empty() {
                continue;
            }
            elements.push(Element {
                block: bi,
                category: 0,
                name,
                sort_name: {
                    let mut s: &[u8] = raw;
                    if let Some(p) = s.iter().position(|&c| c == 0x00) {
                        s = &s[..p];
                    }
                    match std::str::from_utf8(s) {
                        Ok(t) => t.trim().to_string(),
                        Err(_) => s
                            .iter()
                            .map(|&c| c as char)
                            .collect::<String>()
                            .trim()
                            .to_string(),
                    }
                },
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

/// Build an element's display name from the raw trie path, exactly as the device renders it
/// (`LISA_tclListProcessor::vCollectNamesOfCat` / `LISA_tclSortString::vSetStringAndEquivalent`):
/// the accumulator is a C string, so it ends at the first `0x00`; a `<?xml>` description is
/// stripped from display; and the string is a TAB-separated multi-LINE name — line 1 (before the
/// first `0x09`) is the ASCII-folded **sort/type-in key** ("503 NOWODWOR"), while the **display**
/// form is everything after the first tab ("08 503 NOWODWÓR", i.e. incl. the settlement's number
/// and the diacritic-correct/original spelling). `vCollectNamesOfCat` deletes
/// `s32GetHorizontalTabPos()+1` leading bytes (cut line 1 including the tab) before storing the
/// name; without a tab the whole line is the name.
fn decode_name(raw: &[u8]) -> String {
    let mut s: &[u8] = raw;
    if let Some(p) = s.iter().position(|&c| c == 0x00) {
        s = &s[..p];
    }
    if let Some(p) = find_bytes(s, b"<?xml>") {
        s = &s[..p];
    }
    if let Some(p) = s.iter().position(|&c| c == 0x09) {
        s = &s[p + 1..];
    }
    let t = match std::str::from_utf8(s) {
        Ok(t) => t.to_string(),
        Err(_) => s.iter().map(|&c| c as char).collect(),
    };
    t.trim().to_string()
}

fn find_bytes(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
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
    /// Owning city element id (list 2) -> column 0x40c (stock street list fills it; the device
    /// uses it to group streets under a city). `None` = street not listed under any city.
    pub belonging: Option<u32>,
}

struct TrieNode {
    child: Vec<(u8, usize)>, // (edge byte, child handle), insertion order
}

/// Max trie nodes per block. `node_count`/`element_count` are `u16` in the block header, and
/// stock keeps ~11k nodes/block, so we split entries into blocks under this budget.
const MAX_NODES_PER_BLOCK: usize = 10_000;

/// Default identity wrapper around [`encode_id`] (test/oracle convenience).
pub fn encode(entries: &[NameEntry]) -> Vec<u8> {
    encode_id(0, 0, entries)
}

/// Encode name entries into an ASF `LID2nnnn` name-list file (a plain trie, possibly multi-block),
/// the exact byte format `read()` parses back; this is the round-trip oracle for the converter:
/// `read(&encode(&entries)).elements` reproduces the `(name, position)` multiset.
///
/// The file carries the canonical 0x77-byte outer header (`header::nl_header`, kind `1`) with the
/// `rIdxListID` identity `{region, list_id}` — the device identifies a raw list file by these first
/// bytes (stock `LID2nnnn` do exactly this; §11), so they must match the list id used in META
/// relation records.
///
/// Position handling follows the device (§12.5): entries that carry an owning `city` are grouped into
/// blocks **per city** and stored as `position − city` deltas (the device adds the queried city back);
/// the file header then advertises the region's SW corner as its file-level anchor (flags `0x0008_0001`).
/// Entries without a city (the settlement gazetteer) are stored without any coordinate stream (flags
/// `0x0008_0000`, origin `-1/-1`) — exactly like the stock `LID20000`.
pub fn encode_id(region: u16, list_id: u16, entries: &[NameEntry]) -> Vec<u8> {
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

    // --- file sub-header exactly as `NLNameList::LoadHeader` (00e0e63c) demands (§11.4c): u32
    //     element_count, six u16 section counts (A=blocks, D=REL-list ids carried, E=language ids),
    //     F=8, the file origin pair, then 7 {code, offset} section entries whose streams MUST each
    //     consume their span EXACTLY (the device cursor-checks every one). Stock street shape:
    //     sec0=VLE block sizes, sec1=raw block offsets, sec2..4 empty, sec5=VLE [list_id], sec6=VLE [39].
    let linked = list_id == 3; // street lists mirror stock LID20006 (REL id + POL language vector)
    let (d_ids, e_ids): (Vec<u32>, Vec<u32>) = if linked {
        (vec![list_id as u32], vec![39]) // 39 = the card's POL language id (stock LID20006 §11.4c)
    } else {
        (vec![], vec![])
    };
    let mut sec0: Vec<u8> = Vec::new();
    for (blk, _) in &blocks {
        sec0.extend_from_slice(&vle_encode(blk.len() as u32));
    }
    let mut sec5: Vec<u8> = Vec::new();
    for v in &d_ids {
        sec5.extend_from_slice(&vle_encode(*v));
    }
    let mut sec6: Vec<u8> = Vec::new();
    for v in &e_ids {
        sec6.extend_from_slice(&vle_encode(*v));
    }

    let hdr = crate::header::NL_HEADER_LEN as u32;
    let sec1_len = (blocks.len() * 4) as u32;
    let extra: u32 = 59 + sec0.len() as u32 + sec1_len + sec5.len() as u32 + sec6.len() as u32;
    let mut f = crate::header::nl_header(crate::header::KIND_NAME_LIST, region, list_id, extra);
    let mut sh: Vec<u8> = Vec::new();
    sh = put_u32(sh, total_elem as u32); // element_count (global)
    sh = put_u16(sh, blocks.len() as u16); // A
    sh = put_u16(sh, 0); // B
    sh = put_u16(sh, 0); // C
    sh = put_u16(sh, d_ids.len() as u16); // D
    sh = put_u16(sh, e_ids.len() as u16); // E
    sh = put_u16(sh, 8); // F (constant on stock)
    sh = put_u32(sh, corner.0 as u32);
    sh = put_u32(sh, corner.1 as u32); // file-level tNLHPosition (PAU)
    let base = hdr + 59;
    let o1 = base + sec0.len() as u32;
    let o5 = o1 + sec1_len;
    let o6 = o5 + sec5.len() as u32;
    for (code, off) in [
        (0x14u8, base),
        (0x11, o1),
        (0x11, o5),
        (0x11, o5),
        (0x11, o5),
        (0x14, o5),
        (0x14, o6),
    ] {
        sh.push(code);
        sh = put_u32(sh, off);
    }
    assert_eq!(sh.len(), 59);
    f.extend_from_slice(&sh);
    f.extend_from_slice(&sec0);
    let blocks_abs0 = hdr + extra;
    let mut p = blocks_abs0;
    for (blk, _) in &blocks {
        f = put_u32(f, p); // sec1: ABSOLUTE block offsets (stock convention)
        p += blk.len() as u32;
    }
    f.extend_from_slice(&sec5);
    f.extend_from_slice(&sec6);
    assert_eq!(f.len(), blocks_abs0 as usize);
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
            // Prefix edges merge like in any trie. The terminating 0x00 edge NEVER merges:
            // duplicate names are PARALLEL leaf edges — that is how stock stores several
            // same-name elements in one block (CHECKED on POL stock: 843k (name,city) groups
            // with >1 element, up to 105; DEBINY x12; the device enumerates elements per
            // edge, so each duplicate is its own element/coordinate row).
            let mut nxt = None;
            if b != 0 {
                for &(eb, ch) in nodes[h].child.iter() {
                    if eb == b {
                        nxt = Some(ch);
                        break;
                    }
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

    // --- belonging column (0x40c, stock street flavor): per-element owning city element id as
    //     LSB-first existence bitmap (flags 0x4000 row) + VLE values (flags 0 row). The device
    //     groups streets under a city through this column (stock street blocks fill ~3/4 rows). ---
    let mut bel_bits = vec![0u8; (elem_count + 7) / 8];
    let mut bel_vals: Vec<u8> = Vec::new();
    let mut nbel = 0u32;
    for (i, &id) in term.iter().enumerate() {
        if let Some(b) = entries.get(id_to_leaf[id]).and_then(|e| e.belonging) {
            bel_bits[i / 8] |= 1 << (i % 8);
            bel_vals.extend_from_slice(&vle_encode(b));
            nbel += 1;
        }
    }

    // --- numeric/flag streams ---
    let od_bytes = simple9_encode(&outdeg.iter().map(|&d| d as u32).collect::<Vec<u32>>(), 16);
    let loff_bytes: Vec<u8> = loff.iter().fold(Vec::new(), |acc, &o| {
        let mut a = acc;
        a.extend_from_slice(&vle_encode(o));
        a
    });
    // edge -> child-target array (col 0x406, the device's PSF walk source of truth §11.4b).
    // In this plain-trie DFS layout edge #e always lands on node e+1.
    let edge_count = nc.saturating_sub(1);
    let targets: Vec<u32> = (1..=edge_count as u32).collect();
    let targets_bytes = simple9_encode(&targets, 28);
    // mandatory BinList (0x415) stream: stock carries elems u32 Simple9 — all-zero words (mode 1).
    let binlist_bytes = simple9_encode(&vec![0u32; elem_count], 28);

    // --- assemble block: 8B header + descriptors + data, following the STOCK 36-row template
    //     (§11.4b, DAPIAPP enSetListDescriptions/enDecodeAttrLists) so the device's fixed column
    //     set gets a valid description for every always-decoded slot. Columns whose stock data we
    //     cannot synthesize yet are emitted with valid all-clear rows (empty span, param=count). ---
    let (pos_wf, pos_code): (u16, u16) = if anchor.is_some() { (0x4407, 0x03) } else { (0x4407, 0x02) };
    let empty: &[u8] = &[];
    let descs: Vec<(u16, u16, &[u8], u32)> = vec![
        (0x4402, 0x02, empty, nc as u32), // block-link bitmap: none
        (0x0402, 0x14, empty, 0),         // block-link targets: none
        (0x0401, 0x18, &od_bytes, nc as u32), // outDegree Simple9-u16
        (0x4404, 0x02, empty, edge_count as u32),
        (0x8404, 0x11, empty, 0),
        (0x0404, 0x11, empty, 0),
        (0x4405, 0x02, empty, edge_count as u32),
        (0x8405, 0x11, empty, 0),
        (0x0405, 0x11, empty, 0),
        (0x8403, 0x14, &loff_bytes, loff.len() as u32), // edge-label offsets (flags 0x8000)
        (0x0403, 0x11, &blob, blob.len() as u32), // edge-label blob
        (0x0406, 0x18, &targets_bytes, edge_count as u32), // child targets Simple9
        (0x0413, 0x02, empty, edge_count as u32),
        (pos_wf, pos_code, empty, elem_count as u32),
        (
            0x0407,
            0x11,
            &coords,
            if anchor.is_some() { (elem_count * 2) as u32 } else { 0 },
        ),
        (0x440e, 0x02, empty, elem_count as u32),
        (0x040e, 0x11, empty, 0),
        (0x4408, 0x02, empty, elem_count as u32),
        (0x0408, 0x11, empty, 0),
        (0x4409, 0x02, empty, elem_count as u32),
        (0x8409, 0x11, empty, 0),
        (0x0409, 0x11, empty, 0),
        (0x440a, 0x02, empty, elem_count as u32),
        (0x040a, 0x11, empty, 0),
        (0x440b, 0x02, empty, elem_count as u32),
        (0x040b, 0x11, empty, 0),
        (0x440c,
         if anchor.is_some() && nbel > 0 { 0x01 } else { 0x02 },
         if anchor.is_some() && nbel > 0 { &bel_bits } else { empty },
         elem_count as u32), // belonging bitmap (LSB-first, stock row [26] code 0x01)
        if anchor.is_some() && nbel > 0 {
            (0x040c, 0x14, &bel_vals, nbel) // belonging city-element ids (VLE, row [27])
        } else {
            (0x040c, 0x11, empty, 0)
        },
        (0x8415, 0x18, &binlist_bytes, elem_count as u32), // mandatory BinList (zero stream)
        (0x0415, 0x01, empty, 0),
        (0x040f, 0x02, empty, elem_count as u32),
        (0x0410, 0x02, empty, elem_count as u32),
        (0x0411, 0x02, empty, elem_count as u32),
        (0x0414, 0x02, empty, elem_count as u32),
        (0x0412, 0x02, empty, elem_count as u32),
        (0x040d, 0x03, empty, elem_count as u32),
    ];

    let hdr_sz = 8usize + descs.len() * 12;
    let mut block: Vec<u8> = Vec::new();
    block = put_u16(block, nc as u16);
    block = put_u16(block, 1); // f2 = forest root count: each writer block is ONE self-contained trie
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

    #[test]
    fn encode_duplicate_names_roundtrip() {
        // Parallel-terminal-edge model (stock shape): same label + same city must survive
        // encode->decode as SEPARATE elements with their own coordinates, in input order.
        let e = |nm: &str, x: i32, cy: i32| NameEntry {
            label: nm.into(),
            x_pau: x,
            y_pau: 5,
            city: Some((100, cy)),
            belonging: None,
        };
        let entries = vec![
            e("Osada", 10, 1),
            e("X", 20, 1),
            e("Osada", 30, 1), // SAME label AND city as entry 0
            e("Osada", 40, 2), // same label, other city (stock DEBINY pattern)
        ];
        let bytes = encode_id(0x402, 3, &entries);
        let nl = read(&bytes).expect("decode");
        assert_eq!(nl.elements.len(), 4, "duplicates are distinct elements");
        let osada: Vec<_> = nl.elements.iter().filter(|x| x.name == "Osada").collect();
        assert_eq!(osada.len(), 3);
        // Coordinates come back as per-city-anchor deltas (§12.5). City groups keep input order:
        // city1 [Osada(10)->-90, X, Osada(30)->-70], city2 [Osada(40)->-60].
        let names_x: Vec<_> = nl
            .elements
            .iter()
            .map(|x| (x.name.clone(), x.x_pau))
            .filter(|(n, _)| n == "Osada")
            .collect();
        assert_eq!(
            names_x,
            vec![
                ("Osada".into(), -90i32),
                ("Osada".into(), -70),
                ("Osada".into(), -60)
            ]
        );
    }

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
        b.extend_from_slice(&0u32.to_le_bytes()); // this+4 (= filesize − first block_off)
        b.extend_from_slice(&toccount.to_le_bytes());
        let mut off = 0x1000u32;
        for i in 0..toccount {
            let a = i * per_block;
            let c = ((i + 1) * per_block - 1).min(elem_count - 1);
            b.extend_from_slice(&off.to_le_bytes());
            b.extend_from_slice(&a.to_le_bytes());
            b.extend_from_slice(&c.to_le_bytes());
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
        // corrupt the 2nd entry's elem_start (entry table at hdr+12, {off,es,ee}, entry1.es @ +12+12+4)
        let p = 0x77 + 12 + 12 + 4;
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
        // Correct TOC window ⇒ 0xc01 owner domain == block width (the historic −4 grouping read
        // this block's numbers from the NEXT block's table and saw domain 4027 "shifted").
        let c01 = blk
            .streams
            .iter()
            .find(|s| s.col == 0xc01 && s.flags == 0x4000)
            .unwrap();
        assert_eq!(c01.param, 7904);
        // parity bitmap col 0xc09 is per-**record**: its length must equal the 0xc01 value count.
        let par = blk.streams.iter().find(|s| s.col == 0xc09).unwrap();
        let val = blk
            .streams
            .iter()
            .find(|s| s.col == 0xc01 && s.flags == 0)
            .unwrap();
        assert_eq!(par.bits.len(), val.values.len());
        assert_eq!(par.bits.len(), 9665);
        let set = par.bits.iter().filter(|&&x| x).count();
        assert!(
            set > 3000 && set < 7000,
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

    /// Synthetic GenAttr HnR file in the **stock** layout (§11.6b): block ranges are STREET elements,
    /// `0xc01` = per-street house-number lists, `0xc02` per-record ref, `0xc03..0xc06` empty string
    /// lists, `0xc09/0xc0a/0xc0b/0xc0d` per-record parity bits, `0xc11` per-record gate. Write ->
    /// `read_gen_attr`/`decode_block` -> replay `enGetHnrIndices`/`enGetHnr` (`00e0c3a0`/`00e0d078`)
    /// and assert the answers. Proves the writer emits what the card reads, offline.
    #[test]
    fn gen_attr_writer_roundtrip_device_model() {
        use crate::write::{write_gen_attr_file, BlockData, ColData, ColKind};
        let vl = |domain: u32, exists: Vec<bool>, counts: Vec<u32>, values: Vec<u32>| ColData {
            selector: 0xc01,
            kind: ColKind::ValueList,
            domain,
            exists,
            counts,
            values,
            bits: vec![],
            code_8000: 0x16,
            code_0000: 0x14,
            range_from_to: None,
        };
        let gate = |recs: usize| ColData {
            selector: 0xc11,
            kind: ColKind::ValueList,
            domain: recs as u32,
            exists: vec![true; recs],
            counts: (0..recs as u32).collect(),
            values: (0..recs as u32).collect(),
            bits: vec![],
            code_8000: 0x16,
            code_0000: 0x14,
            range_from_to: None,
        };
        let sv = |domain: u32, values: Vec<u32>| ColData {
            selector: 0xc02,
            kind: ColKind::SingleValue,
            domain,
            exists: vec![true; domain as usize],
            counts: vec![],
            values,
            bits: vec![],
            code_8000: 0x16,
            code_0000: 0x14,
            range_from_to: None,
        };
        let bin = |selector: u16, bits: Vec<bool>| ColData {
            selector,
            kind: ColKind::Binary,
            domain: bits.len() as u32,
            exists: vec![],
            counts: vec![],
            values: vec![],
            bits,
            code_8000: 0x16,
            code_0000: 0x14,
            range_from_to: None,
        };
        let es = |selector: u16, recs: u32| ColData {
            selector,
            kind: ColKind::EmptyByteList,
            domain: recs,
            exists: vec![],
            counts: vec![],
            values: vec![],
            bits: vec![],
            code_8000: 0x11,
            code_0000: 0x11,
            range_from_to: None,
        };

        // 6 street elements in 2 tiling blocks: [0..4] (street1 -> {3,4}, street2 -> {7}, 3 records),
        // [5..6] (street5 -> {2}, 1 record).
        let mkb = |e0: u32,
                   e1: u32,
                   nrec: usize,
                   c01: ColData,
                   refs: Vec<u32>,
                   ev: Vec<bool>,
                   od: Vec<bool>| BlockData {
            elem_start: e0,
            elem_end: e1,
            cols: vec![
                c01,
                sv(nrec as u32, refs),
                gate(nrec),
                bin(0xc09, ev),
                bin(0xc0a, od),
                bin(0xc0b, vec![false; nrec]),
                bin(0xc0d, vec![false; nrec]),
                es(0xc03, nrec as u32),
                es(0xc04, nrec as u32),
                es(0xc05, nrec as u32),
                es(0xc06, nrec as u32),
            ],
        };
        let file = write_gen_attr_file(
            7,
            &[],
            &[
                mkb(
                    0,
                    4,
                    3,
                    vl(
                        5,
                        vec![false, true, true, false, false],
                        vec![0, 2],
                        vec![3, 4, 7],
                    ),
                    vec![100, 101, 102],
                    vec![false, true, false], // 3:odd 4:even 7:odd
                    vec![true, false, true],
                ),
                mkb(
                    5,
                    6,
                    1,
                    vl(2, vec![false, true], vec![0], vec![2]),
                    vec![200],
                    vec![true],
                    vec![false],
                ),
            ],
        );

        let gi = read_gen_attr(&file).expect("self-written file must re-parse");
        assert_eq!(gi.element_count, 7);
        assert_eq!(gi.blocks.len(), 2);
        let (ok, mm) = crate::rebuild::rebuild_all(&file);
        assert!(mm.is_none(), "self-written file must byte-rebuild: {mm:?}");
        assert_eq!(ok, 2);

        let blk = gi.decode_block(&file, 0).unwrap();
        let st = |col: u32, flag: u32| {
            blk.streams
                .iter()
                .find(|s| s.col == col && s.flags == flag)
                .unwrap_or_else(|| panic!("missing col {col:03x}/{flag:04x}"))
        };
        // ---- enGetHnrIndices(street elem): before = starts[ordinal], count = next − start. ----
        let hnr_indices = |street_elem: u32| -> Option<(u32, u32)> {
            let ex = st(0xc01, 0x4000).bits.clone();
            let local = (street_elem - blk.elem_start) as usize;
            if local >= ex.len() || !ex[local] {
                return None; // device: no values ⇒ interval, enGetHnr not called
            }
            let ord = ex[..=local].iter().filter(|&&b| b).count() - 1;
            let starts = st(0xc01, 0x8000).values.clone();
            let before = starts[ord];
            let total = st(0xc01, 0).values.len() as u32;
            let end = starts.get(ord + 1).copied().unwrap_or(total);
            Some((before, end - before))
        };
        assert_eq!(hnr_indices(0), None);
        assert_eq!(hnr_indices(1), Some((0, 2)));
        assert_eq!(hnr_indices(2), Some((2, 1)));
        assert_eq!(hnr_indices(3), None);
        assert_eq!(hnr_indices(4), None);

        // ---- enGetHnr(flat record): gate on 0xc11, number from 0xc01, parity from bin cols. ----
        let gate_n = st(0xc11, 0x4000).param;
        assert_eq!(gate_n, 3, "enGetHnr gate must cover all records");
        let nums = st(0xc01, 0).values.clone();
        let ev = st(0xc09, 0).bits.clone();
        let od = st(0xc0a, 0).bits.clone();
        let refs = st(0xc02, 0).values.clone();
        for rec in 0..3usize {
            assert!((rec as u32) < gate_n);
            assert_eq!(nums[rec], [3u32, 4, 7][rec]);
            assert_eq!(ev[rec], [3u32, 4, 7][rec] % 2 == 0);
            assert_eq!(od[rec], [3u32, 4, 7][rec] % 2 == 1);
            assert_eq!(refs[rec], 100 + rec as u32);
        }
        // empty string lists must decode to record-sized all-missing bitmaps (enGetValues succeeds).
        for col in [0xc03u32, 0xc04, 0xc05, 0xc06] {
            let s = st(col, 0x4000);
            assert_eq!(s.param, 3, "{col:03x} existence param");
            assert!(s.bits.iter().all(|&b| !b), "{col:03x} all missing");
        }
    }
}

#[cfg(test)]
mod merge_test {
    use super::*;

    fn u32b(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }
    fn u16b(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
    }
    fn vled(b: &[u8], mut p: usize, n: usize) -> (Vec<u32>, usize) {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let mut acc = 0u32;
            loop {
                let byte = b[p] as u32;
                p += 1;
                acc = if byte & 0x80 != 0 {
                    byte - 0x7f + acc * 128
                } else {
                    byte + acc * 128
                };
                if byte & 0x80 == 0 {
                    break;
                }
            }
            out.push(acc);
        }
        (out, p)
    }

    /// Card experiment "KRZESZOWICE block splice": take the STOCK LID20006 byte-for-byte and append
    /// ONE extra mode-C block with our Krzeszowice streets (global element ids 974871..), rewriting
    /// only the sub-header bookkeeping (elem_count, A, sec0 sizes, sec1 table, extra). Everything
    /// the device already accepts stays untouched — a failure localizes to OUR block content.
    #[test]
    #[ignore = "card experiment: needs FW + krz.tsv"]
    fn splice_krz_block_into_stock() {
        let stock = std::fs::read(
            "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20006.DAT",
        )
        .unwrap();
        let hdr = u32b(&stock, 0x10) as usize;
        let extra = u32b(&stock, 0x14) as usize;
        let elem = u32b(&stock, hdr);
        let a = u16b(&stock, hdr + 4) as usize;
        let corner = (u32b(&stock, hdr + 0x10), u32b(&stock, hdr + 0x14));
        let sec = |i: usize| (stock[hdr + 24 + 5 * i], u32b(&stock, hdr + 25 + 5 * i) as usize);
        let (c0, o0) = sec(0);
        let (c1, o1) = sec(1);
        let o5 = sec(5).1;
        let o6 = sec(6).1;
        assert_eq!((c0, c1), (0x14, 0x11));
        assert_eq!(o5, o1 + 4 * a); // sec2..4 empty
        let sizes_at = stock[o0..o1].to_vec();
        let (sizes, _) = vled(&sizes_at, 0, a);
        let sec5 = stock[o5..o6].to_vec();
        let sec6 = stock[o6..hdr + extra].to_vec();
        let blocks_end = stock.len();

        // our entries (absolute PAU coords in tsv; anchor = stock file corner)
        let tsv = std::fs::read_to_string("/tmp/rnwwork/t27dbg/krz.tsv").unwrap();
        let entries: Vec<NameEntry> = tsv
            .lines()
            .map(|l| {
                let mut it = l.rsplitn(3, '|');
                let y: i32 = it.next().unwrap().parse().unwrap();
                let x: i32 = it.next().unwrap().parse().unwrap();
                let label = it.next().unwrap().replace("\\t", "\t");
                NameEntry {
                    label,
                    x_pau: x,
                    y_pau: y,
                    city: None,
                    belonging: None,
                }
            })
            .collect();
        let anchor = ((corner.0 as i32, corner.1 as i32));
        let (blk, k) = build_block(Some(anchor), &entries);
        assert_eq!(k, entries.len());
        let elem0 = elem + k as u32;

        // new sub-header: sec0 += our size, sec1 += one abs offset, extra shifts everything
        let mut sec0 = sizes_at.clone();
        sec0.extend_from_slice(&vle_encode(blk.len() as u32));
        let no1 = hdr + 59 + sec0.len();
        let no5 = no1 + 4 * (a + 1);
        let nextra = 59 + sec0.len() + 4 * (a + 1) + sec5.len() + sec6.len();
        let nblocks0 = hdr + nextra;

        let mut f = stock[..hdr].to_vec();
        f[0x14..0x18].copy_from_slice(&(nextra as u32).to_le_bytes());
        f.extend_from_slice(&elem0.to_le_bytes());
        f.extend_from_slice(&((a + 1) as u16).to_le_bytes());
        f.extend_from_slice(&stock[hdr + 6..hdr + 24]); // B..F + corner
        for (code, off) in [
            (0x14u8, hdr + 59),
            (0x11, no1),
            (0x11, no5),
            (0x11, no5),
            (0x11, no5),
            (0x14, no5),
            (0x14, no5 + sec5.len()),
        ] {
            f.push(code);
            f.extend_from_slice(&(off as u32).to_le_bytes());
        }
        f.extend_from_slice(&sec0);
        let mut p = nblocks0;
        for sz in sizes.iter().map(|&x| x as usize).chain(std::iter::once(blk.len())) {
            f.extend_from_slice(&(p as u32).to_le_bytes());
            p += sz;
        }
        f.extend_from_slice(&sec5);
        f.extend_from_slice(&sec6);
        assert_eq!(f.len(), nblocks0);
        for i in 0..a {
            let off = u32b(&stock, o1 + 4 * i) as usize;
            let sz = if i + 1 < a {
                u32b(&stock, o1 + 4 * (i + 1)) as usize - off
            } else {
                blocks_end - off
            };
            // NOTE: sizes[] above are byte sizes from sec0; they must match the offsets delta
            assert_eq!(sz, sizes[i] as usize, "stock block size mismatch {i}");
            f.extend_from_slice(&stock[off..off + sz]);
        }
        f.extend_from_slice(&blk);
        std::fs::create_dir_all("/tmp/rnwwork/t27dbg/outG").unwrap();
        std::fs::write("/tmp/rnwwork/t27dbg/outG/LID20006.DAT", &f).unwrap();
        println!("spliced: elem {elem0} (stock {elem} + {k}), A {}, size {:#x}", a + 1, f.len());
        let nl = read(&f).expect("merged file must read back");
        assert_eq!(nl.element_count as u32, elem0);
    }
}
