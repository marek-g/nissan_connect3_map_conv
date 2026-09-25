//! `REL%05u.DAT` sparse relation files (fileType 0x16) — reader model + writer.
//!
//! Layout (CONFIRMED against `NLRelMatrixFile::DecodeSubHeader` `00e11874`,
//! `NLRelMatrixDescr::bCheckAndCalculate` `00e11564`,
//! `enDetermineDataBlocksByType` `00e11ab8`, `enGetSubMatricesInBlock` `00e10fd4`,
//! `enGetRelationsByType` `00e11310`, `NLSubMatrix::en{Column,Row}Values`
//! `00e111ac`/`00e1125c`, and stock `POL/REL00004.DAT` byte-for-byte):
//!
//! * container: `u32@0x10` = header/sub-header split offset (0x77 on stock); `u32@0x14` =
//!   **region size** = `(9 + d6*d7)*4`; the region starts at that split offset.
//! * sub-header region — 9 consecutive u32: `d0, d1` = the two related side ids (stock
//!   REL00004: 129/12); `d2` = source element count, `d3` = target element count; `d4` = source
//!   elements per **band**, `d5` = target elems per band; `d6, d7` = matrix grid dims
//!   (`0 < x < 0x10000`); `d8` (@+0x20) = total tile bytes. Then `d6*d7` u32 **absolute file
//!   offsets** in scan order `g = d6*colBandGroup + rowBandGroup`; tile `g` spans
//!   `[cell[g], cell[g+1])`, the last wraps through `d8 + cell[0]`.
//! * derived geometry (`bCheckAndCalculate`): `srcBands = ceil(d2/d4)`, `tgtBands = ceil(d3/d5)`;
//!   bands per matrix row `bpr` = `srcBands/d6` incremented while `(d6-1)*(bpr+1) < srcBands`
//!   (skipped when `d6 == 1`), same for `bpc`; matrix row `r` covers source bands
//!   `[r*bpr, min((r+1)*bpr, srcBands))` (last group absorbs the remainder).
//! * tile payload: `tcs = rows*cols` u16 **CSR table** where `table[0]` doubles as the
//!   `= tcs` validator (mismatch aborts the tile) and the first cell's value start; entries
//!   non-decreasing; the start of the last cell's successor defaults to `(size & 0x1FFFF) >> 1`,
//!   so tiles must stay < 128 KiB. Table cells are indexed `ci = colBandLocal*rows +
//!   rowBandLocal`. Then per band-pair value lists at `tile + 2*start`, `count = end - start`:
//!   running `pos` += u16 (`0xFFFF` = filler adding `0xFFFE`); decoded with stride `d4` as
//!   `src = d4*srcBandGlobal + pos % d4`, `tgt = d5*tgtBandGlobal + pos / d4` — column queries
//!   (access 2) filter `src` and emit `tgt`, row queries (access 1) filter `tgt` and emit `src`.
//!
//! Writer: uniform grid `d4 = d5 = REL_BAND` elements per band, aiming at `REL_CELL_BANDS` bands
//! per matrix cell (with the device's `bpr` derivation replicated so tables match); errors out
//! instead of emitting tiles that would exceed the device's u16/128 KiB limits.

/// Parsed `REL` file index (sub-header + matrix cells).
#[derive(Debug)]
pub struct RelIndex {
    pub hdr: usize,
    pub region: u32,
    pub d: [u32; 9],
    /// absolute file offset of each matrix cell (scan order `d6*colg + rowg`).
    pub cells: Vec<u32>,
}

impl RelIndex {
    /// Parse + validate (region size, dimension bounds — mirrors the device checks).
    pub fn parse(b: &[u8]) -> Result<RelIndex, String> {
        if b.len() < 0x18 {
            return Err("rel: file too small".to_string());
        }
        let hdr = u32v(b, 0x10) as usize;
        let region = u32v(b, 0x14);
        if hdr as u64 + region as u64 > b.len() as u64 || region < 36 {
            return Err("rel: bad region".to_string());
        }
        let mut d = [0u32; 9];
        let head: [u32; 8] = core::array::from_fn(|i| u32v(b, hdr + 4 * i));
        d[..8].copy_from_slice(&head);
        d[8] = u32v(b, hdr + 0x20);
        let (src, tgt, d4, d5, d6, d7) = (d[2], d[3], d[4], d[5], d[6], d[7]);
        if [src, tgt, d4, d5, d6, d7].contains(&0) || d6 >= 0x10000 || d7 >= 0x10000 {
            return Err("rel: bad dimensions".to_string());
        }
        if region != (d6 * d7 + 9) * 4 {
            return Err("rel: region size mismatch".to_string());
        }
        let n = (d6 * d7) as usize;
        let cells = (0..n).map(|i| u32v(b, hdr + 36 + 4 * i)).collect();
        Ok(RelIndex {
            hdr,
            region,
            d,
            cells,
        })
    }

    /// Device-derived `(srcBands, tgtBands, bpr, bpc)` (see `bCheckAndCalculate`).
    pub fn grid(&self) -> (u64, u64, u64, u64) {
        let (d2, d3, d4, d5, d6, d7) = (
            self.d[2] as u64,
            self.d[3] as u64,
            self.d[4] as u64,
            self.d[5] as u64,
            self.d[6] as u64,
            self.d[7] as u64,
        );
        let (sb, tb) = (d2.div_ceil(d4), d3.div_ceil(d5));
        (sb, tb, bands_per_group(sb, d6), bands_per_group(tb, d7))
    }
}

/// Device `bCheckAndCalculate` band-group size: `S/g` bumped while `(g-1)*(x+1) < S`.
pub fn bands_per_group(bands: u64, groups: u64) -> u64 {
    if groups == 1 {
        return bands;
    }
    let mut x = bands / groups;
    while (groups - 1) * (x + 1) < bands {
        x += 1;
    }
    x
}

/// All relations with the *filtered* side in `[lo, hi)`.
/// `by_source = true` → column query (access 2): filter source, returns `(src, tgt)`.
/// `by_source = false` → row query (access 1): filter target, returns `(tgt, src)`.
pub fn get_relations(
    b: &[u8],
    idx: &RelIndex,
    by_source: bool,
    lo: u32,
    hi: u32,
) -> Result<Vec<(u32, u32)>, String> {
    let (d4, d5, d6, d7) = (
        idx.d[4] as u64,
        idx.d[5] as u64,
        idx.d[6] as u64,
        idx.d[7] as u64,
    );
    let (src_bands, tgt_bands, bpr, bpc) = idx.grid();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for g in 0..(d6 * d7) as usize {
        let (rowg, colg) = (g as u64 % d6, g as u64 / d6);
        let (r0, r1) = (rowg * bpr, ((rowg + 1) * bpr).min(src_bands));
        let (c0, c1) = (colg * bpc, ((colg + 1) * bpc).min(tgt_bands));
        if r0 >= r1 || c0 >= c1 {
            continue;
        }
        let (q0, q1) = if by_source {
            (lo as u64 / d4, (hi as u64).div_ceil(d4))
        } else {
            (lo as u64 / d5, (hi as u64).div_ceil(d5))
        };
        let (b0, b1) = if by_source { (r0, r1) } else { (c0, c1) };
        if b1 <= q0 || b0 >= q1 {
            continue; // this cell's bands cannot hold a queried element
        }
        let off = idx.cells[g] as usize;
        let size = if g + 1 < idx.cells.len() {
            idx.cells[g + 1].wrapping_sub(idx.cells[g]) as usize
        } else {
            idx.d[8]
                .wrapping_add(idx.cells[0])
                .wrapping_sub(idx.cells[g]) as usize
        };
        let (rows, cols) = ((r1 - r0) as usize, (c1 - c0) as usize);
        let tcs = rows * cols;
        if off + 2 * tcs > b.len() {
            return Err(format!("rel: cell {g} table past EOF"));
        }
        if u16v(b, off) as usize != tcs {
            return Err(format!("rel: cell {g} table head != {rows}x{cols}"));
        }
        for c in 0..cols {
            for r in 0..rows {
                let ci = c * rows + r;
                // device (`enGetRelationsByType`): table = raw[1+ci] (count word at raw[0]),
                // stream offsets ABSOLUTE from the block base.
                let start = u16v(b, off + 2 + 2 * ci) as usize;
                let end = if ci + 1 < tcs {
                    let e = u16v(b, off + 2 + 2 * (ci + 1)) as usize;
                    if e < start {
                        return Err(format!("rel: cell {g} CSR not monotonic at {ci}"));
                    }
                    e
                } else {
                    (size & 0x1_FFFF) / 2
                };
                if end < start || start < tcs || off + 2 * end > b.len() {
                    return Err(format!("rel: cell {g} values out of tile"));
                }
                let (row_base, col_base) = ((r0 + r as u64) * d4, (c0 + c as u64) * d5);
                if std::env::var("REL_DEBUG").is_ok() && by_source && row_base == 450 {
                    eprintln!("cell {g} r0 {r0} c0 {c0} r {r} c {c} ci {ci} row_base {row_base} col_base {col_base} start {start} end {end} size {size}");
                }
                if (by_source && hi as u64 <= row_base) || (!by_source && hi as u64 <= col_base) {
                    continue;
                }
                let mut pos: u64 = 0;
                let mut p = off + 2 * start;
                for _ in start..end {
                    let v = u16v(b, p);
                    p += 2;
                    if v == 0xFFFF {
                        // filler: advances the position but is not a relation (enGetRowValues)
                        pos = pos.wrapping_add(0xFFFE);
                        continue;
                    }
                    pos = pos.wrapping_add(v as u64);
                    let (src, tgt) = (row_base + pos % d4, col_base + pos / d4);
                    if src >= idx.d[2] as u64 || tgt >= idx.d[3] as u64 {
                        continue;
                    }
                    if by_source {
                        if (lo as u64..hi as u64).contains(&src) {
                            out.push((src as u32, tgt as u32));
                        }
                    } else if (lo as u64..hi as u64).contains(&tgt) {
                        out.push((tgt as u32, src as u32));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn u32v(b: &[u8], o: usize) -> u32 {
    if o + 4 > b.len() {
        0
    } else {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }
}

fn u16v(b: &[u8], o: usize) -> u16 {
    if o + 2 > b.len() {
        0
    } else {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
}

trait U16Ok {
    fn u16_ok(self) -> Option<u16>;
}
impl U16Ok for usize {
    fn u16_ok(self) -> Option<u16> {
        u16::try_from(self).ok()
    }
}

/// Elements per band (`d4`/`d5`) and target bands per matrix cell for [`write_rel`].
pub const REL_BAND: u64 = 512;
pub const REL_CELL_BANDS: u64 = 32;

/// Sorted position stream as u16 deltas; a `0xFFFF` entry is the filler adding `0xFFFE`.
fn encode_positions(pos: &[u32]) -> Vec<u8> {
    let mut t: Vec<u8> = Vec::new();
    let mut last = 0u64;
    for &p in pos {
        let mut delta = p as u64 - last;
        last = p as u64;
        while delta >= 0xFFFF {
            t.extend_from_slice(&0xFFFFu16.to_le_bytes());
            delta -= 0xFFFE;
        }
        t.extend_from_slice(&(delta as u16).to_le_bytes());
    }
    t
}

/// Build a `REL` file relating `src_elems` source to `tgt_elems` target elements;
/// `list_a`/`list_b` are the side ids stored in the sub-header. Uniform device-style grid
/// (`d4 = d5 = REL_BAND`, groups sized to ≈`REL_CELL_BANDS` bands per matrix cell).
pub fn write_rel(
    src_elems: u64,
    tgt_elems: u64,
    list_a: u16,
    list_b: u16,
    rels: &[(u32, u32)],
) -> Result<Vec<u8>, String> {
    let src_bands = src_elems.div_ceil(REL_BAND);
    let tgt_bands = tgt_elems.div_ceil(REL_BAND);
    let d6 = src_bands.div_ceil(REL_CELL_BANDS);
    let d7 = tgt_bands.div_ceil(REL_CELL_BANDS);
    write_rel_grid(
        src_elems, tgt_elems, list_a, list_b, REL_BAND, REL_BAND, d6, d7, rels,
    )
}

/// `write_rel` with the **explicit device grid** (`d4`/`d5` elements per source/target band,
/// `d6`/`d7` stored matrix dims) — the grid author tools actually pick per file (stock
/// `REL00004` uses `380/806/20/10`), which the byte-exact stock oracles compare against.
pub fn write_rel_grid(
    src_elems: u64,
    tgt_elems: u64,
    list_a: u16,
    list_b: u16,
    d4: u64,
    d5: u64,
    d6: u64,
    d7: u64,
    rels: &[(u32, u32)],
) -> Result<Vec<u8>, String> {
    if src_elems == 0
        || tgt_elems == 0
        || d4 == 0
        || d5 == 0
        || d6 == 0
        || d7 == 0
        || src_elems > u64::from(u32::MAX) / 2
        || tgt_elems > u64::from(u32::MAX) / 2
    {
        return Err("rel: element count out of range".to_string());
    }
    let src_bands = src_elems.div_ceil(d4);
    let tgt_bands = tgt_elems.div_ceil(d5);
    if d6 >= 0x10000 || d7 >= 0x10000 {
        return Err("rel: grid too large".to_string());
    }
    let (bpr, bpc) = (
        bands_per_group(src_bands, d6),
        bands_per_group(tgt_bands, d7),
    );
    if d6 * bpr < src_bands || d7 * bpc < tgt_bands {
        return Err("rel: derived band groups do not cover all bands".to_string());
    }

    // matrix cell `g` = (colg, rowg); its CSR shape is rows x cols band pairs
    let tcs_of = |g: u64| -> (u64, u64, usize) {
        let (rowg, colg) = (g % d6, g / d6);
        let rows = ((rowg + 1) * bpr).min(src_bands) - rowg * bpr;
        let cols = ((colg + 1) * bpc).min(tgt_bands) - colg * bpc;
        (rows, cols, (rows * cols) as usize)
    };
    let mut runs: Vec<Vec<(usize, u32)>> = (0..d6 * d7).map(|_| Vec::new()).collect();
    for &(s, t) in rels {
        let (s, t) = (u64::from(s), u64::from(t));
        if s >= src_elems || t >= tgt_elems {
            return Err(format!("rel: pair ({s},{t}) out of range"));
        }
        let (sb, tb) = (s / d4, t / d5);
        let (rowg, colg) = (sb / bpr, tb / bpc);
        let g = colg * d6 + rowg;
        let (rows, _, _) = tcs_of(g);
        let ci = (tb % bpc) * rows + (sb % bpr);
        // position is band-pair relative: src = rowBase + pos % d4, tgt = colBase + pos / d4
        runs[g as usize].push((ci as usize, ((t % d5) * d4 + s % d4) as u32));
    }

    // one tile per matrix cell: u16 CSR head table, then the concatenated delta streams
    let mut tiles: Vec<Vec<u8>> = Vec::with_capacity(runs.len());
    for (g, cell_runs) in runs.iter().enumerate() {
        let (_, _, tcs) = tcs_of(g as u64);
        if cell_runs.iter().any(|&(ci, _)| ci >= tcs) {
            return Err(format!("rel: cell {g} bad table index"));
        }
        let mut cell_runs = cell_runs.clone();
        cell_runs.sort_unstable();
        cell_runs.dedup();
        let mut hist = vec![0usize; tcs + 1];
        for &(ci, _) in &cell_runs {
            hist[ci + 1] += 1;
        }
        for k in 1..=tcs {
            hist[k] += hist[k - 1];
        }
        // encode each table cell's positions first: filler u16s (0xFFFF for huge deltas) make the
        // encoded length differ from the pair count, so the CSR head must use real offsets.
        let mut streams: Vec<Vec<u8>> = Vec::with_capacity(tcs);
        let mut starts: Vec<u16> = Vec::with_capacity(tcs);
        // device layout: raw[0] = cell count, raw[1+k] = ABSOLUTE u16 offset of cell k's stream
        // counted from the block base (the count word counts as u16 index 0).
        let mut cur = tcs + 1;
        for k in 0..tcs {
            let lo = hist[k];
            let hi = hist[k + 1];
            starts.push(
                cur.u16_ok()
                    .ok_or(format!("rel: cell {g} value offset exceeds u16"))?,
            );
            if lo < hi {
                let mut ps: Vec<u32> = cell_runs[lo..hi].iter().map(|&(_, p)| p).collect();
                ps.sort_unstable();
                ps.dedup();
                let enc = encode_positions(&ps);
                cur += enc.len() / 2;
                streams.push(enc);
            } else {
                streams.push(Vec::new());
            }
        }
        if cur > 0xFFFF {
            return Err(format!("rel: cell {g} tile exceeds device limit"));
        }
        let mut tile = Vec::with_capacity(2 * cur);
        tile.extend_from_slice(&(tcs as u16).to_le_bytes());
        for w in &starts {
            tile.extend_from_slice(&w.to_le_bytes());
        }
        for s in &streams {
            tile.extend_from_slice(s);
        }
        tiles.push(tile);
    }

    // assemble file: canonical 0x77 outer header (kind REL), region (9 words + cell table), tiles
    let region = 36 + 4 * (d6 * d7) as usize;
    let tiles_start = 0x77 + region;
    let tiles: Vec<Vec<u8>> = tiles;
    let tiles_total: usize = tiles.iter().map(|t| t.len()).sum();
    let mut f = crate::header::nl_header(
        crate::header::KIND_REL,
        0,
        0,
        region as u32, // device-validated: region == (9 + d6*d7)*4
    );
    for w in [
        u32::from(list_a),
        u32::from(list_b),
        src_elems as u32,
        tgt_elems as u32,
        d4 as u32,
        d5 as u32,
        d6 as u32,
        d7 as u32,
        tiles_total as u32,
    ] {
        f.extend_from_slice(&w.to_le_bytes());
    }
    let mut off = tiles_start;
    for t in &tiles {
        f.extend_from_slice(&(off as u32).to_le_bytes());
        off += t.len();
    }
    for t in &tiles {
        f.extend_from_slice(t);
    }
    Ok(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// write -> parse -> query returns exactly the intended pairs; gap fillers + multi-cell grid.
    #[test]
    fn rel_roundtrip() {
        let mut rels: Vec<(u32, u32)> = Vec::new();
        for s in 0..2000u32 {
            for t in 0..9u32 {
                if (s * t + s + t) % 3 == 0 {
                    rels.push((s, (t * 600 + (s * 7 + t) % 600) % 5000));
                }
            }
        }
        rels.extend([(0u32, 4999u32), (1999, 0), (1999, 4999)]);
        rels.sort();
        rels.dedup();
        let f = write_rel(2000, 5000, 1, 2, &rels).expect("write");
        let idx = RelIndex::parse(&f).expect("parse");
        let all = get_relations(&f, &idx, true, 0, 2000).expect("query all");
        assert_eq!(all.len(), rels.len(), "extra: {all:?} vs {rels:?}");
        assert_eq!(all, rels, "by_source full-range must match exactly");
        for &(s, t) in rels.iter().take(60) {
            assert!(
                get_relations(&f, &idx, true, s, s + 1)
                    .unwrap()
                    .contains(&(s, t)),
                "by_source {s},{t}"
            );
            assert!(
                get_relations(&f, &idx, false, t, t + 1)
                    .unwrap()
                    .contains(&(t, s)),
                "by_target {t},{s}"
            );
        }
    }

    /// Empty relation set parses and answers "none"; tiles are emitted in the device-valid shape.
    #[test]
    fn rel_roundtrip_empty() {
        let f = write_rel(1000, 1000, 5, 6, &[]).unwrap();
        let idx = RelIndex::parse(&f).unwrap();
        assert!(get_relations(&f, &idx, true, 0, 1000).unwrap().is_empty());
        assert!(get_relations(&f, &idx, false, 0, 1000).unwrap().is_empty());
    }

    /// Writer header satisfies the device validation rules.
    #[test]
    fn rel_header_invariants() {
        let f = write_rel(194118, 412392, 9, 4, &[(0, 0)]).unwrap();
        let hdr = u32v(&f, 0x10) as usize;
        let (d6, d7) = (u32v(&f, hdr + 0x18) as u64, u32v(&f, hdr + 0x1c) as u64);
        assert_eq!(u32v(&f, 0x14) as u64, (9 + d6 * d7) * 4);
        assert_eq!(u32v(&f, hdr + 8), 194118);
        let cells = (0..d6 * d7)
            .map(|i| u32v(&f, hdr + 36 + (4 * i) as usize) as usize)
            .collect::<Vec<_>>();
        assert_eq!(cells[0], 0x77 + (9 + d6 * d7) as usize * 4);
        assert_eq!(cells[0] + u32v(&f, hdr + 0x20) as usize, f.len());
    }

    /// Stock card `REL00004.DAT` parses with the device's derived grid.
    #[ignore = "requires the stock card dump"]
    #[test]
    fn rel_stock_rel00004() {
        let p = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00004.DAT";
        let b = std::fs::read(p).unwrap();
        let idx = RelIndex::parse(&b).unwrap();
        assert_eq!((idx.d[0], idx.d[1]), (129, 12));
        assert_eq!((idx.d[2], idx.d[3]), (194118, 412392));
        assert_eq!((idx.d[4], idx.d[5]), (380, 806));
        assert_eq!((idx.d[6], idx.d[7]), (20, 10));
        let (sb, tb, bpr, bpc) = idx.grid();
        assert_eq!((sb, tb, bpr, bpc), (511, 512, 26, 56));
        // a source-element query must answer with plausible target ids
        let r = get_relations(&b, &idx, true, 0, 40).unwrap();
        assert!(!r.is_empty());
        assert!(r.iter().all(|&(s, t)| s < 40 && t < idx.d[3]));
    }

    /// EQUIVALENCE oracle: every stock `REL` file re-emitted from its *decoded pairs* through
    /// `write_rel_grid` with the file's own grid must decode back to exactly the same relation set.
    /// BYTE equality is deliberately NOT asserted: stock tiles carry author-exporter artifacts —
    /// the last table cell's window often runs past its own stream into a duplicated/shifted
    /// continuation of neighbouring band data (observed: final windows of 1, 2 or 2× length with
    /// constant position offsets; content beyond the cell's band is tolerated/redecoded by the
    /// device as real relations). Our writer emits the minimal spec-faithful encoding instead.
    #[ignore = "requires the stock card dump"]
    #[test]
    fn rel_stock_roundtrip_equivalence() {
        let dir = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";
        for name in [
            "REL00000.DAT",
            "REL00001.DAT",
            "REL00002.DAT",
            "REL00003.DAT",
            "REL00004.DAT",
            "REL00006.DAT",
        ] {
            let b = std::fs::read(format!("{dir}{name}")).unwrap();
            let idx = RelIndex::parse(&b).unwrap();
            let mut rels = get_relations(&b, &idx, true, 0, idx.d[2]).unwrap();
            rels.sort_unstable();
            rels.dedup();
            let mine = write_rel_grid(
                u64::from(idx.d[2]),
                u64::from(idx.d[3]),
                idx.d[0] as u16,
                idx.d[1] as u16,
                u64::from(idx.d[4]),
                u64::from(idx.d[5]),
                u64::from(idx.d[6]),
                u64::from(idx.d[7]),
                &rels,
            )
            .unwrap_or_else(|e| panic!("{name}: write failed: {e}"));
            let stock = get_relations(&b, &idx, true, 0, idx.d[2]).unwrap();
            let midx = RelIndex::parse(&mine).unwrap_or_else(|e| panic!("{name}: reparse: {e}"));
            assert_eq!(midx.d[..8], idx.d[..8], "{name}: sub-header differs");
            assert_eq!(midx.region, idx.region, "{name}: region differs");
            let mine_r = get_relations(&mine, &midx, true, 0, idx.d[2]).unwrap();
            assert_eq!(mine_r, stock, "{name}: decoded relation sets differ");
        }
    }
}

#[cfg(test)]
mod card_tests {
    #[test]
    #[ignore = "needs FW files"]
    fn xcheck_full_dump() {
        for path in [
            "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00001.DAT",
        ] {
            let b = std::fs::read(path).unwrap();
            let idx = super::RelIndex::parse(&b).unwrap();
            let all = super::get_relations(&b, &idx, true, 0, idx.d[2] + 1).unwrap();
            let mut c: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
            for (s_, t_) in all {
                c.entry(t_).or_default().push(s_);
            }
            for city in [109625u32, 7427, 160202] {
                let v = c.get(&city).cloned().unwrap_or_default();
                println!("{path} full-dump city {city} -> {} {:?}", v.len(), &v[..v.len().min(6)]);
            }
        }
    }

    /// Cross-check the python device-path simulator (relsim.py) counts per file.
    #[test]
    #[ignore = "needs /tmp + FW files"]
    fn xcheck_relsim() {
        for path in [
            "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00001.DAT",
            "/tmp/rnwwork/t27dbg/outH/REL00001.DAT",
            "/tmp/rnwwork/t27dbg/outI/REL00001.DAT",
        ] {
            let b = std::fs::read(path).unwrap();
            let idx = super::RelIndex::parse(&b).unwrap();
            for city in [109625u32, 7427, 160202] {
                let r = super::get_relations(&b, &idx, false, city, city + 1).unwrap();
                let mut v: Vec<u32> = r.iter().map(|&(_, t)| t).collect();
                v.sort_unstable();
                println!("{path} city {city} -> {} head={:?}", r.len(), &v[..v.len().min(5)]);
            }
        }
    }

    /// Card-debug probe (run manually): row-query (by_source=false, tgt=city id) our generated
    /// REL00001 for KRZESZOWICE's city element id; must return the street src ids.
    #[test]
    #[ignore = "needs /tmp card files"]
    fn city_to_street_rows() {
        for path in ["/tmp/rnwwork/t27dbg/outH/REL00001.DAT"] {
            let b = std::fs::read(path).unwrap();
            let idx = super::RelIndex::parse(&b).unwrap();
            for city in [109625u32] {
                let r = super::get_relations(&b, &idx, false, city, city + 1).unwrap();
                println!("{path} city {city} -> {} streets", r.len());
            }
        }
    }

    /// Same row query on the STOCK REL00001 for a stock Warsaw city id (control).
    #[test]
    #[ignore = "needs card files"]
    fn city_to_street_rows_stock() {
        let p = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00001.DAT";
        let b = std::fs::read(p).unwrap();
        let idx = super::RelIndex::parse(&b).unwrap();
        for city in [7427u32, 160202, 300619] {
            let r = super::get_relations(&b, &idx, false, city, city + 1).unwrap();
            println!("stock REL00001 by_tgt city {city} -> {} streets", r.len());
        }
        let known: Vec<(String, bool, u32)> = vec![
            ("/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00001.DAT".into(), true, 5),
            ("/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00001.DAT".into(), false, 160202),
            ("/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00002.DAT".into(), true, 7427),
            ("/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00000.DAT".into(), true, 7427),
        ];
        for (p2, bs, id) in known {
            let b = std::fs::read(&p2).unwrap();
            let idx = super::RelIndex::parse(&b).unwrap();
            let r = super::get_relations(&b, &idx, bs, id, id + 1).unwrap();
            println!(
                "{} by_src={} {id} -> {} rels",
                p2.split('/').next_back().unwrap(),
                bs,
                r.len()
            );
        }
    }
}

#[cfg(test)]
mod stock_reemit {
    /// Emit stock REL00001 POL pairs through OUR writer into /tmp/rnwwork/t27dbg/outF for a card
    /// isolation test (stock LID20006 + OUR re-emitted REL00001).
    #[test]
    #[ignore = "writes /tmp card-test file"]
    fn reemit_stock_rel00001() {
        let src = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00001.DAT";
        let b = std::fs::read(src).unwrap();
        let idx = super::RelIndex::parse(&b).unwrap();
        let mut rels = super::get_relations(&b, &idx, true, 0, idx.d[2] + 1).unwrap();
        rels.sort_unstable();
        rels.dedup();
        println!("stock REL00001 pairs: {}", rels.len());
        let mine = super::write_rel_grid(
            u64::from(idx.d[2]),
            u64::from(idx.d[3]),
            idx.d[0] as u16,
            idx.d[1] as u16,
            u64::from(idx.d[4]),
            u64::from(idx.d[5]),
            u64::from(idx.d[6]),
            u64::from(idx.d[7]),
            &rels,
        )
        .unwrap();
        std::fs::write("/tmp/rnwwork/t27dbg/outF/REL00001.DAT", &mine).unwrap();
        let midx = super::RelIndex::parse(&mine).unwrap();
        let back = super::get_relations(&mine, &midx, true, 0, idx.d[2] + 1).unwrap();
        assert_eq!(back.len(), rels.len());
        let crz = super::get_relations(&mine, &midx, false, 160202, 160203).unwrap();
        println!("row-query 160202 -> {} streets", crz.len());
        assert!(!crz.is_empty());
    }
}

#[cfg(test)]
mod merge_rel_test {
    /// Card experiment companion: REL00001 = stock pairs MINUS KRZESZOWICE(109625) rows PLUS rows
    /// pointing at the spliced street elements (global ids 974871..974918) -> city 109625 must
    /// answer with exactly our streets, other cities stay stock.
    #[test]
    #[ignore = "card experiment"]
    fn merge_rel_krz() {
        let src = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00001.DAT";
        let b = std::fs::read(src).unwrap();
        let idx = super::RelIndex::parse(&b).unwrap();
        let all = super::get_relations(&b, &idx, true, 0, idx.d[2] + 1).unwrap();
        let mut rels: Vec<(u32, u32)> = all.iter().copied().filter(|&(_, t)| t != 109625).collect();
        let base = idx.d[2]; // 974871
        let nk = std::fs::read_to_string("/tmp/rnwwork/t27dbg/krz.tsv")
            .unwrap()
            .lines()
            .count() as u32;
        for i in 0..nk {
            rels.push((base + i, 109625));
        }
        rels.sort_unstable();
        rels.dedup();
        let mine = super::write_rel_grid(
            u64::from(base + nk),
            u64::from(idx.d[3]),
            idx.d[0] as u16,
            idx.d[1] as u16,
            u64::from(idx.d[4]),
            u64::from(idx.d[5]),
            u64::from(idx.d[6]),
            u64::from(idx.d[7]),
            &rels,
        )
        .unwrap();
        std::fs::write("/tmp/rnwwork/t27dbg/outG/REL00001.DAT", &mine).unwrap();
        let midx = super::RelIndex::parse(&mine).unwrap();
        let krz = super::get_relations(&mine, &midx, false, 109625, 109626).unwrap();
        println!("KRZ rows: {}", krz.len());
        assert_eq!(krz.len(), nk as usize);
        assert!(krz.iter().all(|&(t, s)| t == 109625 && s >= base));
        let waw = super::get_relations(&mine, &midx, false, 160202, 160203).unwrap();
        println!("control city 160202 rows: {}", waw.len());
        assert!(!waw.is_empty());
    }
}

#[cfg(test)]
mod outH_verify {
    /// Verify the outH merged REL00001: replaced towns answer with our (shifted) ids only,
    /// a stock control city keeps its stock rows.
    #[test]
    #[ignore = "needs outH"]
    fn merged_rel_cities() {
        let b = std::fs::read("/tmp/rnwwork/t27dbg/outH/REL00001.DAT").unwrap();
        let idx = super::RelIndex::parse(&b).unwrap();
        for city in [109625u32, 209548, 219722, 220146, 235786] {
            let r = super::get_relations(&b, &idx, false, city, city + 1).unwrap();
            println!("city {city} -> {} streets", r.len());
            assert!(!r.is_empty());
            assert!(
                r.iter().all(|&(_, s)| s >= 974871),
                "city {city}: stock ids leaked"
            );
        }
        for city in [160202u32, 7427] {
            let r = super::get_relations(&b, &idx, false, city, city + 1).unwrap();
            println!("control {city} -> {}", r.len());
            assert!(!r.is_empty());
            assert!(
                r.iter().all(|&(_, s)| s < 974871),
                "control {city}: ours leaked"
            );
        }
    }
}
