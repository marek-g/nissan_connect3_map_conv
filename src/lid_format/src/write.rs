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

/// One attribute-vector selector triple (existence / counts / values).
pub struct ColData {
    /// Selector (`kind & 0xfff`): e.g. `0xc01`(+0x28 `VL<u32>`  street→addr), `0x002`(SV<u32> addr→street),
    /// `0x00e`(+0x340 `Range<u32>` number from/to), `0xc0a`/`0xc0b`(tBitArray per-addr parity bits).
    pub selector: u16,
    /// Existence domain size (`param` of the `4000` stream; bit count the device's `HasAttribute` tests).
    pub domain: u32,
    /// Existence bits, length `domain` (`true` ⇒ element has this attribute).
    pub exists: Vec<bool>,
    /// Per exist-element list lengths in sorted-exist order (the `8000` stream). Empty for range/SV cols.
    pub counts: Vec<u32>,
    /// The concatenated values (`0000` stream), length = Σ counts. For range/SV: see `range_from_to`.
    pub values: Vec<u32>,
    /// `code` for the `8000` counts stream: `0x16` (cumulative VLE delta) or `0x14` (absolute VLE).
    pub code_8000: u16,
    /// `code` for the `0x0000` values: `0x14` (VLE) or `0x18` (Simple9). Use `0x14` for safety.
    pub code_0000: u16,
    /// For `Range`/`SingleValue` columns: per-exist *(from, to)*; replaces `counts`/`values` and forces
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

/// One block's bytes: `[u16 num] + num*{u16 kind,u16 code,u32 off,u32 param} + stream bytes`. Descriptor
/// stream spans are `[off[i], off[i+1])` in *row order* (row `i+1`'s span ⇒ this one's byte-length), the
// device span rule; rows are packed back-to-back. Param per row mirrors the stock convention.
pub fn build_block(blk: &BlockData) -> Vec<u8> {
    // (kind, code, param, bytes); 3 rows per column: (4000,0x01/0x02), (8000,..), (0000,..).
    let mut rows: Vec<(u16, u16, u32, Vec<u8>)> = Vec::new();
    for c in &blk.cols {
        let ex_code: u16 = if c.exists.iter().any(|&b| b) {
            0x02
        } else {
            0x01
        };
        rows.push((
            c.selector | 0x4000,
            ex_code,
            c.domain,
            exist_bytes(&c.exists, ex_code),
        ));
        let nexist = c.exists.iter().filter(|&&b| b).count() as u32;
        if let Some((from, to)) = &c.range_from_to {
            rows.push((
                c.selector | 0x8000,
                0x16,
                nexist,
                encode_numeric(from, 0x16).expect("range from VLE"),
            ));
            rows.push((
                c.selector,
                0x11,
                to.len() as u32,
                encode_numeric(to, 0x11).expect("range to"),
            ));
        } else {
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

/// Build `LID4nnnn` bytes. Header layout (reader `gen_attr_toc`/`DecodeSubHeader`): `u32 hdr` at
/// `f[0x10]`; the sub-header lives **at `hdr`** = `{u32 elem_count, u32 x, u32 toc_count, u32 other}`,
/// then `hdr+16` = `toc_count × {u32 elem_start,u32 elem_end,u32 block_off(absolute file offset)}`.
/// `outer` = an existing file's first `hdr+16` bytes, reused verbatim; pass `&[]` for a fresh file (a
/// zero `[0..16]`, `hdr=16`, sub-header built from `elem_count` + the block list).
pub fn write_gen_attr_file(element_count: u32, outer: &[u8], blocks: &[BlockData]) -> Vec<u8> {
    let hdr = if outer.len() >= 0x14 {
        u32(outer, 0x10) as usize
    } else {
        0x20
    };
    let toc_bytes = 12 * blocks.len();
    let toc_size = 16 + toc_bytes; // sub-header(16) + TOC at hdr
    let blocks_off = ((hdr + toc_size + 3) & !3).max(hdr + toc_size);
    let mut out: Vec<u8> = vec![0u8; hdr + toc_size]; // sub-header + TOC region; blocks appended below
                                                      // Reuse outer's [0..hdr] (outer container fields incl the `hdr` pointer @0x10) if provided.
    let reuse = hdr.min(outer.len());
    out[..reuse].copy_from_slice(&outer[..reuse]);
    out[0x10..0x14].copy_from_slice(&(hdr as u32).to_le_bytes());
    out[0x14..0x18].copy_from_slice(&(hdr as u32 + toc_size as u32).to_le_bytes()); // sub-header region size
                                                                                    // Sub-header @hdr: elem_count, x (reuse or 0), toc_count, other (reuse or 0).
    if outer.len() >= hdr + 16 {
        out[hdr..hdr + 4].copy_from_slice(&outer[hdr..hdr + 4]);
        out[hdr + 4..hdr + 8].copy_from_slice(&outer[hdr + 4..hdr + 8]);
        out[hdr + 8..hdr + 12].copy_from_slice(&(blocks.len() as u32).to_le_bytes());
        out[hdr + 12..hdr + 16].copy_from_slice(&outer[hdr + 12..hdr + 16]);
    } else {
        out[hdr..hdr + 4].copy_from_slice(&element_count.to_le_bytes());
        out[hdr + 8..hdr + 12].copy_from_slice(&(blocks.len() as u32).to_le_bytes());
    }
    // Blocks.
    let block_bytes: Vec<Vec<u8>> = blocks.iter().map(build_block).collect();
    let mut bo = blocks_off;
    for (i, blk) in blocks.iter().enumerate() {
        let p = hdr + 16 + 12 * i;
        out[p..p + 4].copy_from_slice(&blk.elem_start.to_le_bytes());
        out[p + 4..p + 8].copy_from_slice(&blk.elem_end.to_le_bytes());
        out[p + 8..p + 12].copy_from_slice(&(bo as u32).to_le_bytes());
        bo += block_bytes[i].len();
    }
    out.resize(blocks_off, 0);
    for bb in &block_bytes {
        out.extend_from_slice(bb);
    }
    out
}
