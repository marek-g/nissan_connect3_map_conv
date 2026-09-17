//! GenAttr (`LID4nnnn`) **writer**.
//!
//! Emits author-format blocks that reproduce `read_gen_attr`+`decode_block` byte-for-byte (`rebuild.rs`
//! oracle is the byte proof). Each column is a *triple* of descriptor streams keyed by `(selector,flag)`:
//!
//! * `flag 0x4000` — the vector's **existence bitmap**; its `param` = the column's *domain* count.
//! * `flag 0x8000` — the **cumulative offset** stream (VLE delta of per-existant-element list lengths);
//!   `param` = total length of the `0x0000` values.
//! * `flag 0x0000` — the concatenated **values**.
//!
//! (`SetDescription` decoders consume exactly this triple; `enGetHnr`/`enGetOwner` read the `4000/8000/
//! 0000` fields of the `+0x28`/`+0x7c`/`+0x380`/`+0x4dc` members.) Encoders are the byte-exact ones
//! proven against the author's file in `rebuild.rs`; we reuse them so a block we write re-decodes to our
//! exact input AND `rebuild_all` returns byte-equal output.

use super::{u32, vle_encode};
use crate::rebuild::encode_numeric;

/// Column vector flavor (which descriptor rows the device's `SetDataBlock` decoder expects).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ColKind {
    /// 3 rows: existence / cumulative offsets (VLE delta) / concatenated values
    /// (`NLValueListAttrVector` — `0xc01` house numbers, `0xc11` record domain, …).
    ValueList,
    /// 2 rows: existence / one value per existing *record* (`NLSingleValueAttrVector` — `0xc02` ref).
    SingleValue,
    /// 1 row: raw bit vector; the bits are the data (`NLBinaryAttrVector` — `0xc09`/`0xc0a`/`0xc0b`/`0xc0d`).
    Binary,
    /// The stock `POL` "empty string list" shape for `0xc03..0xc06` (`NLValueListAttrVector<u8>`,
    /// which `enGetHnr` *requires* to decode successfully): existence `0x02` with **zero bytes**
    /// (`param` = record count, all-missing), then two `0x11` rows with `param = 0` (no bytes).
    /// Equal `off` rows are legal (row span = next-off − off, §`decode_block`).
    EmptyByteList,
    /// 3 rows: existence / `from`s / `to`s (`NLRangeAttrVector` — `0xc08`/`0xc10`). `range_from_to`.
    Range,
    /// 1 row: plain numeric list without existence bits — the CellId table member `0x002` (per-row
    /// NAV-file locals; values kept `u16`-wide by convention).
    Simple16,
    /// 1 row: plain numeric list without existence bits — CellId table `0x004` global ids and the
    /// `0x8003/0x8004` record-description starts.
    Simple32,
    /// Verbatim descriptor rows `(selector, code, param, values)` (§11.6c recorddescription stub
    /// `0x4003`; the lost stock writers emitted these with no existence stream).
    Rows(Vec<(u16, u16, u32, Vec<u32>)>),
}

/// One attribute-vector selector triple (existence / offsets / values — flavor per `kind`).
pub struct ColData {
    /// Selector (`kind & 0xfff`): `0xc01`(+0x28 `VL<u32>` street→house numbers), `0xc02`(+0x7c
    /// `SV<u32>` per-record ref), `0xc03..0xc06`(+0xb0.. `VL<u8>` hnr strings — empty on POL),
    /// `0xc09/0xc0a/0xc0b/0xc0d`(+0x260/280/2a0/300 per-record parity bits), `0xc11`(+0x380 `VL<u32>`
    /// per-record existence domain = the `enGetHnr` gate), `0xc10`(+0x340 `Range`).
    pub selector: u16,
    pub kind: ColKind,
    /// Existence domain size (`param` of the `4000` stream; bit count the device's `HasAttribute` /
    /// `GetNumberOfExistenceFlags` tests). For `ValueList`/`SingleValue`/`EmptyByteList`.
    pub domain: u32,
    /// Existence bits, length `domain` (`true` ⇒ owner/element has this attribute).
    pub exists: Vec<bool>,
    /// Per exist-element **absolute start offsets** in the flat `values` list in sorted-exist order,
    /// first = 0 (the `8000` stream, `0x16`-delta encoded ⇒ decodes back to absolutes, like stock).
    /// Unused by `SingleValue`/`Binary`/`EmptyByteList`/`Range`.
    pub counts: Vec<u32>,
    /// The concatenated values (`0000` stream), length = for `ValueList` Σ counts (abs starts, last
    /// implicit = list end). For `SingleValue`: one value per existing record.
    pub values: Vec<u32>,
    /// Bit data for `Binary` columns (length = record count).
    pub bits: Vec<bool>,
    /// `code` for the `8000` counts stream: `0x16` (cumulative VLE delta) or `0x14` (absolute VLE).
    pub code_8000: u16,
    /// `code` for the `0x0000` values: `0x14` (VLE) or `0x18` (Simple9). Use `0x14` for safety.
    pub code_0000: u16,
    /// For `Range` columns: per-exist *(from, to)*; replaces `counts`/`values` and forces
    /// `8000`=`0x16` froms & `0000`=`0x11` tos.
    pub range_from_to: Option<(Vec<u32>, Vec<u32>)>,
}

/// A full GenAttr block's logical content (address-element domain `elem_start..elem_end`).
pub struct BlockData {
    pub elem_start: u32,
    pub elem_end: u32,
    pub cols: Vec<ColData>,
}

/// Existence bitmap bytes from `exists` (`0x01` dense / `0x02` sparse-set; both decode identical bits).
fn exist_bytes(exists: &[bool], code: u16) -> Vec<u8> {
    match code {
        0x02 => { let mut o = Vec::new(); let mut acc = 0u32; for (i, &b) in exists.iter().enumerate() { if b { let p = i as u32; o.extend(vle_encode(p - acc)); acc = p } } o }
        0x03 => { let mut o = Vec::new(); let mut acc = 0u32; for (i, &b) in exists.iter().enumerate() { if !b { let p = i as u32; o.extend(vle_encode(p - acc)); acc = p } } o }
        _ /* 0x01 dense */ => {
            let mut by = vec![0u8; (exists.len() + 7) / 8];
            for (i, &b) in exists.iter().enumerate() { if b { by[i / 8] |= 1 << (i % 8) } }
            by
        }
    }
}

/// Existence bitmap `code` matching the stock convention: `0x01` dense (none set ⇒ zero-filled),
/// `0x02` sparse set-positions (mixed), `0x03` sparse unset-positions (all set ⇒ zero bytes).
fn ex_code(exists: &[bool]) -> u16 {
    let t = exists.iter().filter(|&&b| b).count();
    if t == 0 {
        0x01
    } else if t == exists.len() {
        0x03
    } else {
        0x02
    }
}

/// One block's bytes: `[u16 num] + num*{u16 kind,u16 code,u32 off,u32 param} + stream bytes`. Descriptor
/// stream spans are `[off[i], off[i+1])` in *row order* (row `i+1`'s span ⇒ this one's byte-length), the
// device span rule; rows are packed back-to-back. `0`-length rows are legal (equal `off`), e.g. the
// stock EmptyByteList existence + offset rows. Param per row mirrors the stock convention.
pub fn build_block(blk: &BlockData) -> Vec<u8> {
    // (kind, code, param, bytes) — rows per column depend on the vector `kind`.
    let mut rows: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
    for c in &blk.cols {
        match &c.kind {
            ColKind::ValueList => {
                let xc = ex_code(&c.exists);
                rows.push((
                    c.selector | 0x4000,
                    xc,
                    c.domain,
                    exist_bytes(&c.exists, xc),
                ));
                rows.push((
                    c.selector | 0x8000,
                    c.code_8000,
                    c.counts.len() as u32,
                    encode_numeric(&c.counts, c.code_8000 as u32).expect("counts"),
                ));
                rows.push((
                    c.selector,
                    c.code_0000,
                    c.values.len() as u32,
                    encode_numeric(&c.values, c.code_0000 as u32).expect("values"),
                ));
            }
            ColKind::Range => {
                let (from, to) = c
                    .range_from_to
                    .as_ref()
                    .expect("Range column needs range_from_to");
                let xc = ex_code(&c.exists);
                rows.push((
                    c.selector | 0x4000,
                    xc,
                    c.domain,
                    exist_bytes(&c.exists, xc),
                ));
                rows.push((
                    c.selector | 0x8000,
                    0x16,
                    from.len() as u32,
                    encode_numeric(from, 0x16).expect("range from VLE"),
                ));
                rows.push((
                    c.selector,
                    0x11,
                    to.len() as u32,
                    encode_numeric(to, 0x11).expect("range to"),
                ));
            }
            ColKind::SingleValue => {
                let xc = ex_code(&c.exists);
                rows.push((
                    c.selector | 0x4000,
                    xc,
                    c.domain,
                    exist_bytes(&c.exists, xc),
                ));
                rows.push((
                    c.selector,
                    c.code_0000,
                    c.values.len() as u32,
                    encode_numeric(&c.values, c.code_0000 as u32).expect("values"),
                ));
            }
            ColKind::Binary => {
                rows.push((
                    c.selector,
                    0x01,
                    c.bits.len() as u32,
                    exist_bytes(&c.bits, 0x01),
                ));
            }
            ColKind::EmptyByteList => {
                // stock POL shape: `0x02` existence with 0 bytes, then `0x11`/`0x11` with param 0.
                rows.push((c.selector | 0x4000, 0x02, c.domain, Vec::new()));
                rows.push((c.selector | 0x8000, 0x11, 0, Vec::new()));
                rows.push((c.selector, 0x11, 0, Vec::new()));
            }
            ColKind::Simple16 | ColKind::Simple32 => {
                let bad = match c.kind {
                    ColKind::Simple16 => c.values.iter().any(|&v| v > 0xFFFF),
                    _ => false,
                };
                assert!(!bad, "Simple16 column {:#x} has a >u16 value", c.selector);
                rows.push((
                    c.selector,
                    c.code_0000,
                    c.values.len() as u32,
                    encode_numeric(&c.values, c.code_0000 as u32).expect("simple values"),
                ));
            }
            ColKind::Rows(extra) => {
                for (sel, code, param, vals) in extra.clone() {
                    // an empty payload skips the codec entirely (stock `0x03` stubs carry none)
                    let bytes = if vals.is_empty() {
                        Vec::new()
                    } else {
                        encode_numeric(&vals, code as u32).expect("row values")
                    };
                    rows.push((sel, code, param, bytes));
                }
            }
        }
    }
    let n = rows.len() as u16;
    let hdr = 2 + 12 * n as usize;
    let mut out: Vec<u8> = Vec::with_capacity(hdr + rows.iter().map(|r| r.3.len()).sum::<usize>());
    out.extend_from_slice(&n.to_le_bytes());
    let mut off = hdr as u32;
    let mut body: Vec<u8> = Vec::new();
    for (kind, code, param, bytes) in &rows {
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&code.to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
        out.extend_from_slice(&param.to_le_bytes());
        off += bytes.len() as u32;
        body.extend_from_slice(bytes);
    }
    out.extend_from_slice(&body);
    out
}

/// Build `LID4nnnn` bytes. Header layout (reader `gen_attr_toc`/`DecodeSubHeader` `00e0e3d0`):
/// `u32 hdr` at `f[0x10]`; the sub-header lives **at `hdr`** = `{u32 elem_count, u32 x = total block
/// bytes (= filesize − first block_off), u32 toc_count}`, then the `NLBlockTocEntry` table **at
/// `hdr+12`** = `toc_count × {u32 block_off (absolute), u32 elem_start, u32 elem_end}` (this is the
/// field order `GetBlockDescr` 00e0dd4c consumes; the last block runs to EOF). `outer` = an existing
/// file's first `hdr+16` bytes, reused verbatim; pass `&[]` for a fresh file (a zero `[0..16]`,
/// `hdr=16`, sub-header built from `elem_count` + the block list).
pub fn write_gen_attr_file(element_count: u32, outer: &[u8], blocks: &[BlockData]) -> Vec<u8> {
    let hdr = if outer.len() >= 0x14 {
        u32(outer, 0x10) as usize
    } else {
        0x20
    };
    let toc_bytes = 12 * blocks.len();
    let toc_size = 16 + toc_bytes; // sub-header + TOC (table starts at hdr+12, pads to hdr+16+12n)
    let blocks_off = ((hdr + toc_size + 3) & !3).max(hdr + toc_size);
    let mut out: Vec<u8> = vec![0u8; hdr + toc_size]; // sub-header + TOC region; blocks appended below
                                                      // Reuse outer's [0..hdr] (outer container fields incl the `hdr` pointer @0x10) if provided.
    let reuse = hdr.min(outer.len());
    out[..reuse].copy_from_slice(&outer[..reuse]);
    out[0x10..0x14].copy_from_slice(&(hdr as u32).to_le_bytes());
    out[0x14..0x18].copy_from_slice(&(hdr as u32 + toc_size as u32).to_le_bytes()); // sub-header region size
                                                                                    // Sub-header @hdr: elem_count, x (reuse or computed), toc_count.
    out[hdr..hdr + 4].copy_from_slice(&element_count.to_le_bytes());
    out[hdr + 8..hdr + 12].copy_from_slice(&(blocks.len() as u32).to_le_bytes());
    // Blocks.
    let block_bytes: Vec<Vec<u8>> = blocks.iter().map(build_block).collect();
    let mut bo = blocks_off;
    for (i, blk) in blocks.iter().enumerate() {
        let p = hdr + 12 + 12 * i;
        out[p..p + 4].copy_from_slice(&(bo as u32).to_le_bytes());
        out[p + 4..p + 8].copy_from_slice(&blk.elem_start.to_le_bytes());
        out[p + 8..p + 12].copy_from_slice(&blk.elem_end.to_le_bytes());
        bo += block_bytes[i].len();
    }
    let total: usize = block_bytes.iter().map(|b| b.len()).sum();
    out[hdr + 4..hdr + 8].copy_from_slice(&(total as u32).to_le_bytes());
    out.resize(blocks_off, 0);
    for bb in &block_bytes {
        out.extend_from_slice(bb);
    }
    out
}

/// One `LID3%05u` crossing block: the element range plus the column-`0x801` `NLValueListAttrVector`
/// content (`exists` over the block's element range, `starts` = abs offsets into `values`,
/// `values` = partner street element-ids; a partner may repeat once per distinct meeting node,
/// never the row's own element — all stock-DEU-verified shape rules).
pub struct CrossingBlockData {
    pub elem_start: u32,
    pub elem_end: u32,
    pub exists: Vec<bool>,
    pub starts: Vec<u32>,
    pub values: Vec<u32>,
}

/// Build one crossing block. Emits the stock 0x801 triple first (`0x4801` existence, `0x8801`
/// VLE-delta `0x16` starts, `0x0801` VLE `0x14` values — the codecs `write.rs` proves byte-exact)
/// and then the stock's per-value-slot *empty* columns (`0x803`/`0x804` empty VLs, `0x807`/`0x808`
/// existence-only vectors with no bits, the `0x802` all-present/implicit VL triple) as
/// zero-byte rows with stock codes/params, so the descriptor table mirrors stock block layouts.
/// The not-yet-reversed columns are intentionally NOT emitted: `0x806` (unknown VL), `0x8001`
/// (crossing status stream, `00e077cc` undecoded) and the `NLCellIdAttrVector` cell table
/// `0x001..0x005` (crossing-side semantics unreversed, LID_format.md §11 note); the device's
/// per-column decode gates skip absent vectors. [OPEN] crossing positions therefore cannot be
/// served until the cell columns are written too.
pub fn build_crossing_block(blk: &CrossingBlockData) -> Vec<u8> {
    let domain = blk.exists.len() as u32;
    let v = blk.values.len() as u32;
    let xc = ex_code(&blk.exists);
    let rows: Vec<(u16, u16, u32, Vec<u8>)> = vec![
        (0x4801, xc, domain, exist_bytes(&blk.exists, xc)),
        (
            0x8801,
            0x16,
            blk.starts.len() as u32,
            encode_numeric(&blk.starts, 0x16).expect("starts"),
        ),
        (
            0x0801,
            0x14,
            v,
            encode_numeric(&blk.values, 0x14).expect("values"),
        ),
        (0x4803, 0x02, v, Vec::new()),
        (0x0803, 0x11, 0, Vec::new()),
        (0x4804, 0x02, v, Vec::new()),
        (0x0804, 0x11, 0, Vec::new()),
        (0x0807, 0x02, v, Vec::new()),
        (0x0808, 0x02, v, Vec::new()),
        (0x4802, 0x03, v, Vec::new()),
        (0x8802, 0x17, v, Vec::new()),
        (0x0802, 0x18, v, Vec::new()),
    ];
    let n = rows.len() as u16;
    let hdr = 2 + 12 * n as usize;
    let mut out: Vec<u8> = Vec::with_capacity(hdr + rows.iter().map(|r| r.3.len()).sum::<usize>());
    out.extend_from_slice(&n.to_le_bytes());
    let mut off = hdr as u32;
    let mut body: Vec<u8> = Vec::new();
    for (kind, code, param, bytes) in &rows {
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&code.to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
        out.extend_from_slice(&param.to_le_bytes());
        off += bytes.len() as u32;
        body.extend_from_slice(bytes);
    }
    out.extend_from_slice(&body);
    out
}

/// Build `LID3%05u` bytes (modern gen, header kind 2). Sub-header at `hdr` =
/// `{u32 elem_count, u32 total_values, u32 block_bytes, u32 block_count}` then the same
/// `NLBlockTocEntry` table as GenAttr at `hdr+16` (`NLCrossingFile::DecodeSubHeader 00e08eb0`;
/// DEU `LID30006` field probe: `total_values` = Σ row lengths, `block_bytes` = filesize − first
/// block offset). `elem_count` must equal the sibling street name-list element count (stock
/// invariant: `LID3` and `LID2` share the element-id domain). `outer` = canonical header bytes.
pub fn write_crossing_file(
    element_count: u32,
    outer: &[u8],
    blocks: &[CrossingBlockData],
) -> Vec<u8> {
    let hdr = if outer.len() >= 0x14 {
        u32(outer, 0x10) as usize
    } else {
        0x20
    };
    let toc_size = 16 + 12 * blocks.len();
    let blocks_off = ((hdr + toc_size + 3) & !3).max(hdr + toc_size);
    let mut out: Vec<u8> = vec![0u8; hdr + toc_size];
    let reuse = hdr.min(outer.len());
    out[..reuse].copy_from_slice(&outer[..reuse]);
    out[0x10..0x14].copy_from_slice(&(hdr as u32).to_le_bytes());
    out[0x14..0x18].copy_from_slice(&(hdr as u32 + toc_size as u32).to_le_bytes());
    let block_bytes: Vec<Vec<u8>> = blocks.iter().map(build_crossing_block).collect();
    let mut bo = blocks_off;
    for (i, blk) in blocks.iter().enumerate() {
        let p = hdr + 16 + 12 * i;
        out[p..p + 4].copy_from_slice(&(bo as u32).to_le_bytes());
        out[p + 4..p + 8].copy_from_slice(&blk.elem_start.to_le_bytes());
        out[p + 8..p + 12].copy_from_slice(&blk.elem_end.to_le_bytes());
        bo += block_bytes[i].len();
    }
    let total: u32 = blocks.iter().map(|b| b.values.len() as u32).sum();
    out[hdr..hdr + 4].copy_from_slice(&element_count.to_le_bytes());
    out[hdr + 4..hdr + 8].copy_from_slice(&total.to_le_bytes());
    out[hdr + 8..hdr + 12].copy_from_slice(&((bo - blocks_off) as u32).to_le_bytes());
    out[hdr + 12..hdr + 16].copy_from_slice(&(blocks.len() as u32).to_le_bytes());
    out.resize(blocks_off, 0);
    for bb in &block_bytes {
        out.extend_from_slice(bb);
    }
    out
}

#[cfg(test)]
mod crossing_tests {
    use super::*;
    use crate::{header, read_crossings};

    #[test]
    fn crossing_roundtrip() {
        let blocks = vec![
            CrossingBlockData {
                elem_start: 0,
                elem_end: 2,
                exists: vec![true, false, true],
                starts: vec![0, 2],
                values: vec![42, 7, 42],
            },
            CrossingBlockData {
                elem_start: 3,
                elem_end: 5,
                exists: vec![false, false, false],
                starts: vec![],
                values: vec![],
            },
        ];
        let outer = header::nl_header(header::KIND_CROSSING, 99, 3, 0);
        let bytes = write_crossing_file(6, &outer, &blocks);
        assert!(crate::is_crossing(&bytes));
        assert!(!crate::is_name_list(&bytes));
        let ci = read_crossings(&bytes).unwrap();
        assert_eq!(ci.element_count, 6);
        assert_eq!(ci.blocks.len(), 2);
        let b0 = ci.decode_crossings(&bytes, 0).unwrap();
        assert_eq!(b0.len(), 2);
        assert_eq!(b0[0].element, 0);
        assert_eq!(b0[0].streets, vec![42, 7]);
        assert_eq!(b0[1].element, 2);
        assert_eq!(b0[1].streets, vec![42]);
        assert!(ci.decode_crossings(&bytes, 1).unwrap().is_empty());
    }
}
