//! Point-address (`PA_%05u`, fileType `0x18`) container: `NLPAFile` + `NLPADetailBlock`.
//!
//! NO stock card ships a `PA_*.DAT` (verified: zero files across the whole CCP card), so this module
//! cannot be byte-validated against an author file; it is derived from the device decode path —
//! `NLPAFile::DecodeSubHeader 00ce012c` + `FillBlockDescription 00ce0078` + `GetBlockDescr 00cdfa14`
//! + `NLPAProcessor::bGetDetails 00ce98f0` + `NLPADetailBlock::SetDataBlock 00e0f8c4` /
//! `bDecodeLists 00e0f824` — and validated by replaying that path ([`pa_device_model`] test).
//!
//! File layout (`hdr = u32@0x10`; the 38-byte outer header is shared with the other NL files):
//! ```text
//! @hdr:  u32 A(domain element count), u32 B(reserved 0), u16 coord_mode, u16 group_count(=1)
//! @+12:  group_count × { u16 list (0xd01|0xd02|0xd03), u16 flags,
//!                        u32 entry_table_off (FILE-absolute), u32 entry_count, u32 byte_span }
//! @entry_table_off: entry_count × { u32 elem_width, u32 block_off (FILE-absolute) }
//! @block_off: u32 elem_width, u16 desc_count, desc_count × 11-byte PACKED { u16 kind, u8 code,
//!                 u32 off (BLOCK-relative), u32 param }, then stream bytes in *row order*
//! ```
//! Blocks of one group tile the element domain contiguously (device walk [`PaIndex::locate`]): the
//! first block covers `[0, w0-1]`, then `end_i = end_{i-1} + w_i`. A block's byte size is
//! `next_off - off`; the last is `byte_span + first_off - off`.
//!
//! The DETAIL list (`0xd03`, keyed by **street element** — what `LISA_tclHnrProcessing::bGetPACells
//! 00be072c` queries via `NLPAProcessor::bGetDetails(street_elem)`; accessors
//! `u32GetCellIndex/bIsOnLeftHandSide/…` all index by `elem - block.start`) carries per element:
//! cell id (`0xd0b` `SimpleList<u32>`), on-left (`0xd0c` Binary), on-right (`0xd0d` Binary),
//! access-point ratio (`0xd0e` `SimpleList<u8>`, 0..100 %; the device NLHnr value = pct·255/100),
//! and a RELATIVE position (`0xd0f` `NLPositionAttrVector`; `bGetPACells` ADDS the street element's
//! own name-list position — so we store `house_pau − street_anchor_pau`). The `cell` value is NOT a
//! global id: `bGetPACellIDs 00b898dc` feeds detail `+0x00` into `NLGenAttrProcessor::bGetCellOfBlock
//! 00ce6458` → `enGetCells` of the street's own `+20000` block, so it must be a `0xc11` table ordinal
//! of that street's GenAttr file (LID_format §11.6b). The cell/ratio existence
//! bitmaps are written all-set (`0x03`, zero bytes): those two lists are indexed by *element*, so a
//! sparse encoding would mis-rank them; the position bitmap is the only sparse one.

use crate::rebuild::encode_numeric;
use crate::{bitfield, decode_u32, u16, u32, vle_encode};

/// One DETAIL-block element slot's data (street element = domain position).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaDetailEntry {
    pub cell: u32,
    pub left: bool,
    pub right: bool,
    /// 0..=100 percent along the street to the access point (device NLHnr status = pct·255/100).
    pub ratio: u8,
    /// Relative PAU position (`Some`) vs "no position" (`None` — device returns 0x7fffffff → interpolate).
    pub pos: Option<(i32, i32)>,
}

/// One DETAIL block; width = `entries.len()`, tiling continues from the previous block.
pub struct PaDetailBlock {
    pub entries: Vec<PaDetailEntry>,
}

/// A sub-header list group (one of the three `tPASubHeaderListToc`).
#[derive(Debug, Clone)]
pub struct PaList {
    pub list: u16, // selector: 0xd01 / 0xd02 / 0xd03
    pub flags: u16,
    pub table_off: u32,
    pub entry_count: u32,
    pub byte_span: u32,
    /// per block: (elem_width, file-absolute block offset)
    pub entries: Vec<(u32, u32)>,
}

/// Parsed `PA_%05u` sub-header + group tables.
#[derive(Debug, Clone)]
pub struct PaIndex {
    pub hdr: usize,
    pub domain: u32,
    /// the u16 fed to `NLPADetailBlock::bDecodeLists` [`OPEN: exact device semantics`]
    pub coord_mode: u16,
    pub lists: Vec<PaList>,
}

impl PaIndex {
    pub fn detail_list(&self) -> Option<&PaList> {
        self.lists.iter().find(|l| l.list == 0xd03)
    }

    /// Replay `NLPAFile::GetBlockDescr 00cdfa14`: element → (block_off, block_size, elem_start, width).
    pub fn locate(list: &PaList, elem: u32) -> Option<(usize, usize, u32, u32)> {
        let mut end: i64 = 0;
        for (i, &(w, off)) in list.entries.iter().enumerate() {
            let e = if i == 0 {
                (w as i64) - 1
            } else {
                end + w as i64
            };
            if (elem as i64) <= e {
                let size: i64 = if i + 1 < list.entries.len() {
                    list.entries[i + 1].1 as i64 - off as i64
                } else {
                    list.byte_span as i64 + list.entries[0].1 as i64 - off as i64
                };
                let start = if i == 0 { 0u32 } else { (end + 1) as u32 };
                return Some((off as usize, size.max(0) as usize, start, w));
            }
            end = e;
        }
        None
    }
}

/// Parse + structure-validate (`DecodeSubHeader` + `FillBlockDescription` bounds).
pub fn parse_pa(b: &[u8]) -> Result<PaIndex, String> {
    let n = b.len();
    let hdr = u32(b, 0x10) as usize;
    if hdr < 0x20 || hdr + 12 > n {
        return Err("pa: bad sub-header offset".into());
    }
    let domain = u32(b, hdr);
    let coord_mode = u16(b, hdr + 8) as u16;
    let ng = u16(b, hdr + 10) as usize;
    if ng > 3 {
        return Err("pa: group count > 3".into());
    }
    let mut lists = Vec::with_capacity(ng);
    for i in 0..ng {
        let p = hdr + 12 + 16 * i;
        if p + 16 > n {
            return Err("pa: truncated group entry".into());
        }
        let sel = (u16(b, p) & 0xfff) as u16;
        let flags = u16(b, p + 2) as u16;
        let table_off = u32(b, p + 4);
        let cnt = u32(b, p + 8);
        let span = u32(b, p + 12);
        if !matches!(sel, 0xd01..=0xd03) {
            return Err(format!("pa: unknown list kind {sel:#x}"));
        }
        // FillBlockDescription's exact bound check
        if (table_off as u64) + (cnt as u64) * 8 > n as u64 {
            return Err("pa: block table past EOF".into());
        }
        let mut entries = Vec::with_capacity((cnt as usize).min(1 << 20));
        let mut prev_off: u32 = 0;
        for k in 0..cnt {
            let q = table_off as usize + 8 * k as usize;
            let w = u32(b, q);
            let off = u32(b, q + 4);
            if w == 0 || off < hdr as u32 || (k > 0 && off < prev_off) {
                return Err("pa: bad block entry (width/off order)".into());
            }
            prev_off = off;
            entries.push((w, off));
        }
        lists.push(PaList {
            list: sel,
            flags,
            table_off,
            entry_count: cnt,
            byte_span: span,
            entries,
        });
    }
    Ok(PaIndex {
        hdr,
        domain,
        coord_mode,
        lists,
    })
}

/// Decode one DETAIL block (`NLPADetailBlock::SetDataBlock + bDecodeLists`).
pub fn decode_pa_detail_block(b: &[u8], off: usize, size: usize) -> Result<PaDetailBlock, String> {
    let be = (off + size).min(b.len());
    if off + 6 > be {
        return Err("pa: detail block truncated".into());
    }
    let width = u32(b, off) as usize;
    let nd = u16(b, off + 4) as usize;
    struct Row {
        kind: u32,
        flags: u32,
        code: u32,
        off: u32,
        param: u32,
    }
    let mut rows: Vec<Row> = Vec::with_capacity(nd);
    let mut p = off + 6;
    while rows.len() < nd {
        if p + 11 > be {
            return Err("pa: descriptor table truncated".into());
        }
        let k = u16(b, p) as u32;
        rows.push(Row {
            kind: k & 0xfff,
            flags: k & 0xf000,
            code: b[p + 2] as u32, // {u16 kind, u8 code, u32 off, u32 param} packed — device cursor 11B/row
            off: u32(b, p + 3),
            param: u32(b, p + 7),
        });
        p += 11;
    }
    let span_end = |i: usize| -> usize {
        if i + 1 < rows.len() {
            off + rows[i + 1].off as usize
        } else {
            be
        }
    };
    let row = |kind: u32, flags: u32| -> Option<usize> {
        rows.iter().position(|r| r.kind == kind && r.flags == flags)
    };
    let values = |i: usize, count: usize| -> Vec<u32> {
        let r = &rows[i];
        decode_u32(
            b,
            r.code,
            (off + r.off as usize).min(be),
            span_end(i).min(be),
            count,
        )
    };
    let bits = |kind: u32| -> Vec<bool> {
        match row(kind, 0) {
            Some(i) => {
                let r = &rows[i];
                bitfield(
                    b,
                    r.code,
                    (off + r.off as usize).min(be),
                    span_end(i).min(be),
                    width,
                )
            }
            None => vec![false; width],
        }
    };
    let cells = match row(0xd0b, 0) {
        // `param` = the declared u32 count (device bDecodeLists consumes it, not the width).
        Some(i) => values(i, rows[i].param as usize),
        None => Vec::new(),
    };
    if cells.len() != width {
        return Err("pa: cell list width mismatch".into());
    }
    let left = bits(0xd0c);
    let right = bits(0xd0d);
    let ratio: Vec<u8> = match row(0xd0e, 0) {
        Some(i) => {
            let r = &rows[i];
            let (s, e) = ((off + r.off as usize).min(be), span_end(i).min(be));
            match r.code {
                // Raw stream of u8 (simple-list of bytes): one byte per element.
                0x11 => b[s..e.min(s + width)].to_vec(),
                _ => values(i, width).iter().map(|&v| v as u8).collect(),
            }
        }
        None => vec![100u8; width],
    };
    if ratio.len() != width || left.len() != width || right.len() != width {
        return Err("pa: detail list width mismatch".into());
    }
    let pos_bits = match row(0xd0f, 0x4000) {
        Some(i) => {
            let r = &rows[i];
            bitfield(
                b,
                r.code,
                (off + r.off as usize).min(be),
                span_end(i).min(be),
                width,
            )
        }
        None => vec![false; width],
    };
    let pos_vals: Vec<u32> = match row(0xd0f, 0) {
        Some(i) => values(i, rows[i].param as usize),
        None => Vec::new(),
    };
    let mut entries = Vec::with_capacity(width);
    let mut rank = 0usize;
    for k in 0..width {
        let pos = if pos_bits[k] {
            let x = *pos_vals.get(rank * 2).ok_or("pa: pos x missing")? as i32;
            let y = *pos_vals.get(rank * 2 + 1).ok_or("pa: pos y missing")? as i32;
            rank += 1;
            Some((x, y))
        } else {
            None
        };
        entries.push(PaDetailEntry {
            cell: cells[k],
            left: left[k],
            right: right[k],
            ratio: ratio[k],
            pos,
        });
    }
    Ok(PaDetailBlock { entries })
}

fn dense_bits(bits: &[bool]) -> Vec<u8> {
    let mut by = vec![0u8; (bits.len() + 7) / 8];
    for (i, &bit) in bits.iter().enumerate() {
        if bit {
            by[i / 8] |= 1 << (i % 8)
        }
    }
    by
}

fn exist_code(bits: &[bool]) -> u8 {
    let t = bits.iter().filter(|&&b| b).count();
    if t == 0 {
        0x01
    } else if t == bits.len() {
        0x03
    } else {
        0x02
    }
}

fn exist_bytes(bits: &[bool], code: u8) -> Vec<u8> {
    match code {
        0x02 => {
            let mut o = Vec::new();
            let mut acc = 0u32;
            for (i, &bit) in bits.iter().enumerate() {
                if bit {
                    let p = i as u32;
                    o.extend(vle_encode(p - acc));
                    acc = p
                }
            }
            o
        }
        0x03 => {
            let mut o = Vec::new();
            let mut acc = 0u32;
            for (i, &bit) in bits.iter().enumerate() {
                if !bit {
                    let p = i as u32;
                    o.extend(vle_encode(p - acc));
                    acc = p
                }
            }
            o
        }
        _ => dense_bits(bits),
    }
}

struct RowSpec {
    kind: u16, // full 16-bit (flags<<12 | selector)
    code: u8,
    param: u32,
    bytes: Vec<u8>,
}

/// Emit `PA_%05u.DAT` (DETAIL list only, group_count = 1). Blocks tile the domain from element 0;
/// `region_size` in the outer header covers the sub-header + entry tables (the device
/// `FillBlockDescription` bounds check), while blocks are lazily read by absolute offset.
/// `region`/`list_id` go into the outer header; the file name uses the *base* list fileID
/// (`bGetFileNameFromDataAddress 00bc4d54` fileType 0x18 ⇒ street list ⇒ `PA_20006.DAT` for POL).
pub fn write_pa_file(
    region: u16,
    list_id: u16,
    domain: u32,
    coord_mode: u16,
    blocks: &[PaDetailBlock],
) -> Vec<u8> {
    assert!(!blocks.is_empty(), "pa: need at least one block");
    let hdr = crate::header::NL_HEADER_LEN;
    let table_off = (hdr + 12 + 16) as u32; // after sub-header + the single group entry
    let n = blocks.len();

    let mut built: Vec<Vec<u8>> = Vec::with_capacity(n);
    for blk in blocks {
        let w = blk.entries.len();
        assert!(w > 0 && w <= u16::MAX as usize * 4, "pa: block width");
        let cells: Vec<u32> = blk.entries.iter().map(|e| e.cell).collect();
        let cell_bytes = encode_numeric(&cells, 0x11).expect("cells raw");
        let left: Vec<bool> = blk.entries.iter().map(|e| e.left).collect();
        let right: Vec<bool> = blk.entries.iter().map(|e| e.right).collect();
        let ratio: Vec<u8> = blk.entries.iter().map(|e| e.ratio).collect();
        let pos_bits: Vec<bool> = blk.entries.iter().map(|e| e.pos.is_some()).collect();
        let mut pos_bytes: Vec<u8> = Vec::new();
        for e in &blk.entries {
            if let Some((x, y)) = e.pos {
                pos_bytes.extend_from_slice(&(x as u32).to_le_bytes());
                pos_bytes.extend_from_slice(&(y as u32).to_le_bytes());
            }
        }
        let pc = pos_bits.iter().filter(|&&x| x).count();
        let pc_code = exist_code(&pos_bits);
        let rows = [
            RowSpec {
                kind: 0x4d0b,
                code: 0x03,
                param: w as u32,
                bytes: Vec::new(),
            }, // cells exist all-set
            RowSpec {
                kind: 0x0d0b,
                code: 0x11,
                param: w as u32,
                bytes: cell_bytes,
            },
            RowSpec {
                kind: 0x0d0c,
                code: 0x01,
                param: w as u32,
                bytes: dense_bits(&left),
            },
            RowSpec {
                kind: 0x0d0d,
                code: 0x01,
                param: w as u32,
                bytes: dense_bits(&right),
            },
            RowSpec {
                kind: 0x4d0e,
                code: 0x03,
                param: w as u32,
                bytes: Vec::new(),
            }, // ratio exist all-set
            RowSpec {
                kind: 0x0d0e,
                code: 0x11,
                param: w as u32,
                bytes: ratio,
            },
            RowSpec {
                kind: 0x4d0f,
                code: pc_code,
                param: w as u32,
                bytes: exist_bytes(&pos_bits, pc_code),
            },
            RowSpec {
                kind: 0x0d0f,
                code: 0x11,
                param: (2 * pc) as u32,
                bytes: pos_bytes,
            },
        ];
        let nd = rows.len() as u16;
        let tbl = 4 + 2 + 11 * rows.len();
        let mut bb: Vec<u8> =
            Vec::with_capacity(tbl + rows.iter().map(|r| r.bytes.len()).sum::<usize>());
        bb.extend_from_slice(&(w as u32).to_le_bytes());
        bb.extend_from_slice(&nd.to_le_bytes());
        let mut ro = tbl as u32;
        for r in &rows {
            bb.extend_from_slice(&r.kind.to_le_bytes());
            bb.push(r.code); // packed {u16 kind, u8 code, u32 off, u32 param} — device cursor 11B/row
            bb.extend_from_slice(&ro.to_le_bytes());
            bb.extend_from_slice(&r.param.to_le_bytes());
            ro += r.bytes.len() as u32;
        }
        for r in &rows {
            bb.extend_from_slice(&r.bytes);
        }
        built.push(bb);
    }

    let mut offs: Vec<u32> = Vec::with_capacity(n);
    let mut bo = (table_off as usize + 8 * n + 3) & !3;
    for bb in &built {
        offs.push(bo as u32);
        bo = (bo + bb.len() + 3) & !3;
    }
    let first = offs[0];
    let end_last = offs[n - 1] as u64 + built[n - 1].len() as u64;
    let span = (end_last - first as u64) as u32;
    let final_len = offs[n - 1] + built[n - 1].len() as u32;

    let regionsz = (table_off as usize + 8 * n) - hdr;
    let mut file =
        crate::header::nl_header(crate::header::KIND_PA, region, list_id, regionsz as u32);
    file.resize(hdr + 12 + 16, 0);
    file[hdr..hdr + 4].copy_from_slice(&domain.to_le_bytes());
    file[hdr + 4..hdr + 8].copy_from_slice(&0u32.to_le_bytes());
    file[hdr + 8..hdr + 10].copy_from_slice(&coord_mode.to_le_bytes());
    file[hdr + 10..hdr + 12].copy_from_slice(&1u16.to_le_bytes());
    file[hdr + 12..hdr + 14].copy_from_slice(&0xd03u16.to_le_bytes());
    file[hdr + 14..hdr + 16].copy_from_slice(&0u16.to_le_bytes());
    file[hdr + 16..hdr + 20].copy_from_slice(&table_off.to_le_bytes());
    file[hdr + 20..hdr + 24].copy_from_slice(&(n as u32).to_le_bytes());
    file[hdr + 24..hdr + 28].copy_from_slice(&span.to_le_bytes());
    file.resize(table_off as usize, 0);
    for i in 0..n {
        file.extend_from_slice(&(blocks[i].entries.len() as u32).to_le_bytes());
        file.extend_from_slice(&offs[i].to_le_bytes());
    }
    for (i, bb) in built.iter().enumerate() {
        file.resize(offs[i] as usize, 0);
        file.extend_from_slice(bb);
    }
    debug_assert_eq!(file.len() as u32, final_len);
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(k: usize) -> PaDetailEntry {
        PaDetailEntry {
            cell: k as u32 * 7,
            left: k % 3 == 0,
            right: k % 4 == 1,
            ratio: (k % 101) as u8,
            pos: if k % 2 == 0 {
                Some((-(k as i32), k as i32 * 1000))
            } else {
                None
            },
        }
    }

    /// Writer -> `parse_pa` + [`PaIndex::locate`] (device walk) -> `decode_pa_detail_block`, plus the
    /// `bGetPACells`-level semantics (per-street answers exact; domain overflow falls back).
    #[test]
    fn pa_device_model() {
        let blocks: Vec<PaDetailBlock> = vec![
            PaDetailBlock {
                entries: (0..5).map(entry).collect(),
            },
            PaDetailBlock {
                entries: (5..11).map(entry).collect(),
            },
        ];
        let f = write_pa_file(2, 3, 11, 1, &blocks);
        let idx = parse_pa(&f).unwrap();
        assert_eq!(idx.domain, 11);
        let dl = idx.detail_list().expect("detail list");
        let mut cache: Option<(usize, PaDetailBlock)> = None;
        for e in 0..11u32 {
            let (off, size, start, width) = PaIndex::locate(dl, e).expect("locate");
            if cache.as_ref().map_or(true, |c| c.0 != off) {
                cache = Some((off, decode_pa_detail_block(&f, off, size).expect("decode")));
            }
            let blk = &cache.as_ref().unwrap().1;
            assert!((start..start + width).contains(&e), "walk range {e}");
            assert_eq!(
                blk.entries[(e - start) as usize],
                entry(e as usize),
                "elem {e}"
            );
        }
        // block ranges tile the domain: [0..4], [5..10]
        let b0 = PaIndex::locate(dl, 0).unwrap();
        assert_eq!((b0.2, b0.3), (0, 5));
        // device fallback path (no PA row ⇒ interpolation)
        assert!(PaIndex::locate(dl, 11).is_none());
        // `bGetPACells` coordinate math: final = rel + street-element anchor
        let rel = entry(6).pos.expect("pos");
        let anchor = (100_000i32, -50_000i32);
        assert_eq!((anchor.0 + rel.0, anchor.1 + rel.1), (99_994, -44_000));
    }

    /// Structural validation rejects corrupt tables (device bounds).
    #[test]
    fn pa_rejects_garbage() {
        // not enough bytes
        assert!(parse_pa(&[0u8; 0x20]).is_err());
        // sub-header pointer out of bounds
        let mut f = write_pa_file(
            2,
            3,
            4,
            1,
            &[PaDetailBlock {
                entries: (0..4).map(entry).collect(),
            }],
        );
        let hdr = u32(&f, 0x10) as usize;
        f[hdr + 16..hdr + 20].copy_from_slice(&(80_000_000u32).to_le_bytes()); // table_off ⇒ EOF
        assert!(parse_pa(&f).is_err());
    }
}
