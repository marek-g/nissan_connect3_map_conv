//! Device-level diagnostic simulator (`nl_sim`) for the city-selection screen.
//!
//! Faithful transcription of the DAPIAPP (card image `/DAPIAPP.OUT`) flow gates:
//! * `NLFileBase::LoadHeader` 00ce08d8 + `NLRelMatrixFile::DecodeSubHeader` 00e11874 +
//!   `NLRelMatrixDescr::bCheckAndCalculate` 00e11564 - REL file admission gates.
//! * `NLRelationProcessor::bGetRelationsPerIndex` 00cf1f28 -> `NLRelMatrixBlock`
//!   00e10fd4 (sub-matrix geometry) -> `NLSubMatrix::enGetColumnValues` 00e111ac
//!   (delta stream decode).  Every `return 0` device gate is a hard `Err(Gate)` here.
//! * `LISA_tclAddressSearch::vPopulateCityIndices` 00c73b68 - candidate gate model:
//!   district ids -> REL00003 column query -> city ids -> `bGoToElemet` + language gate
//!   + name/position collection (keyboard letters are derived from the surviving names).
//!
//! The simulator is diagnostic: every gate logs WHY it fired, so a screen symptom
//! (empty list / empty keyboard) maps to exactly one failing gate.

use crate::rel::bands_per_group;
use crate::{read, Element, NameList};

/// A failing device gate (the device aborts the whole query when any of these fire).
#[derive(Debug, Clone)]
pub struct Gate(pub String);

impl Gate {
    fn fail<T>(msg: impl Into<String>) -> Result<T, Gate> {
        Err(Gate(msg.into()))
    }
}

/// Device-faithful REL file model (`NLRelationProcessor` over `REL%05u.DAT`).
pub struct RelSim<'a> {
    pub name: String,
    b: &'a [u8],
    hdr: usize,
    /// d0..d8 exactly as `bCheckAndCalculate` reads them at descr +0x00..+0x20.
    pub d: [u32; 9],
    /// computed geometry (descr +0x20..): src_bands, tgt_bands, bpr, bpc.
    pub geo: [u64; 4],
    cells: Vec<u32>,
}

impl<'a> RelSim<'a> {
    /// `NLFileBase::LoadHeader` + `DecodeSubHeader` + `bCheckAndCalculate`, gate-faithful.
    pub fn open(name: &str, b: &'a [u8]) -> Result<RelSim<'a>, Gate> {
        if b.len() < 0x77 + 36 {
            return Gate::fail("LoadHeader: file smaller than header+region");
        }
        let kind = u32le(b, 0x0c);
        if kind != 6 {
            return Gate::fail(format!("LoadHeader: file kind {kind:#x} != 6 (REL)"));
        }
        let hdr = u32le(b, 0x10) as usize;
        let region = u32le(b, 0x14) as usize;
        if hdr + region > b.len() {
            return Gate::fail("LoadHeader: sub-header region past EOF");
        }
        let mut d = [0u32; 9];
        for (i, w) in d[..9].iter_mut().enumerate() {
            *w = u32le(b, hdr + 4 * i);
        }
        // bCheckAndCalculate 00e11564: nonzero d2..d5, d6/d7 in [1, 0x10000)
        if d[2] == 0 || d[3] == 0 || d[4] == 0 || d[5] == 0 {
            return Gate::fail(format!("bCheckAndCalculate: zero dimension d2..d5 = {d:?}"));
        }
        if d[6] == 0 || d[6] >= 0x10000 || d[7] == 0 || d[7] >= 0x10000 {
            return Gate::fail(format!(
                "bCheckAndCalculate: grid dims d6/d7 out of range: {d:?}"
            ));
        }
        let src_bands = (u64::from(d[2]) + u64::from(d[4]) - 1) / u64::from(d[4]);
        let tgt_bands = (u64::from(d[3]) + u64::from(d[5]) - 1) / u64::from(d[5]);
        let bpr = bands_per_group(src_bands, u64::from(d[6]));
        let bpc = bands_per_group(tgt_bands, u64::from(d[7]));
        // DecodeSubHeader: param_2 == (d6*d7 + 9)*4
        if region != ((d[6] as usize) * (d[7] as usize) + 9) * 4 {
            return Gate::fail(format!(
                "DecodeSubHeader: region {region} != (d6*d7+9)*4 = {}",
                ((d[6] as usize) * (d[7] as usize) + 9) * 4
            ));
        }
        let nc = (d[6] as usize) * (d[7] as usize);
        let cells: Vec<u32> = (0..nc).map(|i| u32le(b, hdr + 36 + 4 * i)).collect();
        for (i, &c) in cells.iter().enumerate() {
            if (c as usize) < hdr + region || (c as usize) > b.len() {
                return Gate::fail(format!(
                    "DecodeSubHeader: cell {i} offset {c} outside payload [{}, {}]",
                    hdr + region,
                    b.len()
                ));
            }
            if i > 0 && c < cells[i - 1] {
                return Gate::fail(format!(
                    "DecodeSubHeader: cell offsets not monotonic at {i}"
                ));
            }
        }
        Ok(RelSim {
            name: name.to_string(),
            b,
            hdr,
            d,
            geo: [src_bands, tgt_bands, bpr, bpc],
            cells,
        })
    }

    /// `bGetRelationsPerIndex(access, ids, bySource=true)` - column query, gate-faithful.
    /// Returns `(src, tgt)` pairs; ANY device `return 0` becomes `Err(Gate)`.
    pub fn per_index_by_source(&self, ids: &[u32]) -> Result<Vec<(u32, u32)>, Gate> {
        let (d4, d5) = (u64::from(self.d[4]), u64::from(self.d[5]));
        let (d6, d7) = (u64::from(self.d[6]), u64::from(self.d[7]));
        let (src_bands, tgt_bands, bpr, bpc) = (self.geo[0], self.geo[1], self.geo[2], self.geo[3]);
        let mut out: Vec<(u32, u32)> = Vec::new();

        for &id in ids {
            if u64::from(id) >= u64::from(self.d[2]) {
                return Gate::fail(format!(
                    "enDetermineDataBlocks: query id {id} >= src count {}",
                    self.d[2]
                ));
            }
            let qb = u64::from(id) / d4; // COLUMN access: band = src / d4
                                         // find the cell group whose row-band window contains qb
            let mut cells_to_scan: Vec<(u64, u64, u64, u64, u64)> = Vec::new();
            for rg in 0..d6 {
                let (r0, r1) = (rg * bpr, ((rg + 1) * bpr).min(src_bands));
                if r0 <= qb && qb < r1 {
                    for cg in 0..d7 {
                        let (c0, c1) = (cg * bpc, ((cg + 1) * bpc).min(tgt_bands));
                        if c0 < c1 {
                            cells_to_scan.push((cg * d6 + rg, r0, r1, c0, c1));
                        }
                    }
                    break;
                }
            }
            if cells_to_scan.is_empty() {
                return Gate::fail(format!(
                    "enGetSubMatricesInBlock: band {qb} (id {id}) not covered by any cell group"
                ));
            }
            for (g, r0, _r1, c0, c1) in cells_to_scan {
                let off = self.cells[g as usize] as usize;

                let size = if g as usize + 1 < self.cells.len() {
                    (self.cells[g as usize + 1] - self.cells[g as usize]) as usize
                } else {
                    (u64::from(self.d[8]) + u64::from(self.cells[0])
                        - u64::from(self.cells[g as usize])) as usize
                };
                if size > 0x2_0000 {
                    return Gate::fail(format!(
                    "bGetRelationsPerIndex: tile {g} size {size} exceeds read limit (gate this+0xd4)"
                ));
                }
                let (rows, cols) = ((_r1 - r0) as u64, (c1 - c0) as u64);
                let tcs = (rows * cols) as usize;
                if off + 2 * (tcs + 1) > self.b.len() {
                    return Gate::fail(format!("enGetRelationsByType: tile {g} table past EOF"));
                }
                if std::env::var("NLSIM_DEBUG").is_ok() {
                    eprintln!("DBG id {id}: g {g} r0 {r0} c0 {c0} c1 {c1} off {off} size {size} tcs {tcs} head {}", u16le(self.b, off));
                }
                if u16le(self.b, off) as usize != tcs {
                    return Gate::fail(format!(
                        "enGetRelationsByType: tile {g} table head {} != rows*cols {tcs}",
                        u16le(self.b, off)
                    ));
                }
                for c in 0..cols as usize {
                    for r in 0..rows as usize {
                        let ci = c * rows as usize + r;
                        let start = u16le(self.b, off + 2 + 2 * ci) as usize;
                        let end = if ci + 1 < tcs {
                            let e = u16le(self.b, off + 2 + 2 * (ci + 1)) as usize;
                            if e < start {
                                return Gate::fail(format!(
                                "enGetRelationsByType: tile {g} cell {ci} offsets not monotonic"
                            ));
                            }
                            e
                        } else {
                            (size & 0x1_FFFF) / 2
                        };
                        if end < start || start < tcs || off + 2 * end > self.b.len() {
                            return Gate::fail(format!(
                                "enGetRelationsByType: tile {g} cell {ci} stream out of tile"
                            ));
                        }
                        let (row_base, col_base) = ((r0 + r as u64) * d4, (c0 + c as u64) * d5);
                        let mut pos: u64 = 0;
                        let mut p = off + 2 * start;
                        for _ in start..end {
                            let v = u16le(self.b, p);
                            p += 2;
                            if v == 0xFFFF {
                                pos = pos.wrapping_add(0xFFFE);
                                continue;
                            }
                            pos = pos.wrapping_add(u64::from(v));
                            let (src, tgt) = (row_base + pos % d4, col_base + pos / d4);
                            if std::env::var("NLSIM_DEBUG").is_ok() && src == u64::from(id) {
                                eprintln!("DBG hit id {id} tgt {tgt}");
                            }
                            if src == u64::from(id) && tgt < u64::from(self.d[3]) {
                                out.push((id, tgt as u32));
                            }
                        }
                    }
                }
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}

/// One city candidate as surfaced to the UI after the `vPopulateCityIndices` gates.
#[derive(Debug)]
pub struct Candidate {
    pub id: u32,
    pub name: String,
    pub lon: f64,
    pub lat: f64,
}

/// City-screen model: district ids -> REL00003 -> city elements passing all gates.
pub struct CityScreen<'a> {
    pub city: NameList,
    pub city_shift: u16,
    pub origin: (i32, i32),
    pub rel3: RelSim<'a>,
    /// UI language index gate (u8GetCurrUILangIndex); our files carry lang 0 only.
    pub ui_lang: u8,
}

impl<'a> CityScreen<'a> {
    pub fn new(
        city_bytes: &'a [u8],
        rel3_bytes: &'a [u8],
        ui_lang: u8,
    ) -> Result<CityScreen<'a>, Gate> {
        let city = read(city_bytes).map_err(Gate)?;
        let sh = crate::pos_shift(city_bytes);
        let origin = city
            .origin
            .ok_or_else(|| Gate("city file has no position origin (-1/-1)".to_string()))?;
        let rel3 = RelSim::open("REL00003", rel3_bytes)?;
        Ok(CityScreen {
            city,
            city_shift: sh,
            origin,
            rel3,
            ui_lang,
        })
    }

    /// `vPopulateCityIndices`: districts -> surviving city candidates with names + WGS84.
    /// Returns (district, candidates) with the per-gate failure log attached.
    pub fn populate(&self, districts: &[u32]) -> (Vec<(u32, Vec<Candidate>)>, Vec<String>) {
        let mut log = Vec::new();
        let mut out = Vec::new();
        let pairs = match self.rel3.per_index_by_source(districts) {
            Ok(p) => p,
            Err(Gate(g)) => {
                log.push(format!("REL00003 GATE FAIL: {g}"));
                out.push((0u32, Vec::new()));
                return (out, log);
            }
        };
        for &d in districts {
            let mut cands = Vec::new();
            for (s, t) in pairs.iter().filter(|(s, _)| *s == d) {
                // bGoToElemet gate
                let Some(el) = self.city.elements.get(*t as usize) else {
                    log.push(format!(
                        "district {d}: city id {t} REJECTED by bGoToElemet (>= elem count {})",
                        self.city.element_count
                    ));
                    continue;
                };
                let e: &Element = el;
                // enGetAllElementProperties: local_1f0 (belonging) must be -1
                if e.belonging != 0xFFFF_FFFF {
                    log.push(format!(
                        "district {d}: city {t} \"{}\" REJECTED: belonging != -1",
                        e.name
                    ));
                    continue;
                }
                // language gate: UI lang must be supported (our template: only lang 0)
                if self.ui_lang != 0 {
                    log.push(format!(
                        "district {d}: city {t} \"{}\" REJECTED by lang gate (ui={})",
                        e.name, self.ui_lang
                    ));
                    continue;
                }
                let sh = self.city_shift;
                let lon = crate::pau_to_deg(self.origin.0 + (e.x_pau << sh));
                let lat = crate::pau_to_deg(self.origin.1 + (e.y_pau << sh));
                cands.push(Candidate {
                    id: *t,
                    name: e.name.clone(),
                    lon,
                    lat,
                });
            }
            out.push((d, cands));
        }
        (out, log)
    }

    /// Keyboard letters = first letters of the surviving candidate names (sorted).
    pub fn letters(cands: &[&Candidate]) -> String {
        let mut l: Vec<char> = cands
            .iter()
            .filter_map(|c| c.name.chars().next())
            .filter(|c| !c.is_whitespace())
            .collect();
        l.sort_unstable();
        l.dedup();
        l.into_iter().collect()
    }
}

fn u32le(b: &[u8], o: usize) -> u32 {
    if o + 4 > b.len() {
        0
    } else {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }
}

fn u16le(b: &[u8], o: usize) -> u16 {
    if o + 2 > b.len() {
        0
    } else {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
}

/// Decoded name-list subheader (file kind 0x13/0x14/0x15), as `NLNameList::LoadHeader`
/// 00e0e63c reads it (verified layout, 2026-09-25):
/// `+0 elem_count, +2 flag, +4 nb, +6 nrec, +8 nrel, +0x0a cnt5, +0x0c cnt6,
///  +0x0e shift, +0x10/0x14 origin, +0x18 7x{u8 codec, u32 file offset}`.
#[derive(Debug, Clone)]
pub struct NlHeader {
    pub hdr: usize,
    pub blob_end: usize,
    pub elem: u16,
    pub nb: u16,
    pub nrec: u16,
    pub nrel: u16,
    pub shift: u16,
    pub origin: (i32, i32),
    pub streams: Vec<Vec<u32>>,
    pub rel_files: Vec<u16>,
    pub cats: Vec<u16>,
}

/// Device-faithful `NLNameList::LoadHeader`: reads the 0x26 container bytes, then the
/// subheader at `hdr` and decodes the 7 header streams with the device's exact rules:
/// codes 0x11..0x17 must yield exactly `count` values AND consume the stream window to
/// the last byte; code 0x18 (Simple9) fills a vector sized by `count` (extra packed
/// values after the vector is full are read and discarded) and fails only on a bad mode
/// nibble or on premature end-of-window.  Any violation = the device rejects the file
/// (`bInitialise` -> 0) and the address search aborts => empty city list + dead keyboard.
pub fn check_name_list_header(name: &str, b: &[u8]) -> Result<NlHeader, Gate> {
    if b.len() < 0x26 {
        return Gate::fail(format!("{name}: LoadHeader: container shorter than 0x26"));
    }
    let hdr = u32le(b, 0x10) as usize;
    let reg = u32le(b, 0x14) as usize;
    let blob_end = hdr + reg;
    if blob_end > b.len() {
        return Gate::fail(format!(
            "{name}: LoadHeader: hdr {hdr} + region {reg} = {blob_end} > file size {}",
            b.len()
        ));
    }
    let h = hdr;
    let hd = |o: usize| u16le(b, h + o) as usize;
    let (elem, nb, nrec, nrel) = (u16le(b, h), hd(4) as u16, hd(6) as u16, hd(8) as u16);
    let (cnt5, cnt6, shift) = (hd(0x0a), hd(0x0c), u16le(b, h + 0x0e));
    let origin = (u32le(b, h + 0x10) as i32, u32le(b, h + 0x14) as i32);
    let mut desc = [(0u8, 0usize); 7];
    for i in 0..7 {
        desc[i] = (b[h + 0x18 + i * 5], u32le(b, h + 0x19 + i * 5) as usize);
    }
    for i in 0..7 {
        if desc[i].1 > blob_end || (i + 1 < 7 && desc[i + 1].1 < desc[i].1) {
            return Gate::fail(format!(
                "{name}: LoadHeader: stream {} window {}..{} invalid (blob_end {blob_end})",
                i,
                desc[i].1,
                if i + 1 < 7 { desc[i + 1].1 } else { blob_end }
            ));
        }
    }
    let counts = [nb, nb, nrec, nrec, nrel, cnt5 as u16, cnt6 as u16];
    let mut streams = Vec::new();
    let mut rel_files = Vec::new();
    let mut cats = Vec::new();
    for i in 0..7 {
        let (code, cnt) = (desc[i].0, counts[i] as usize);
        let (s, e) = (desc[i].1, if i + 1 < 7 { desc[i + 1].1 } else { blob_end });
        let (vals, pos, strict) = if i < 4 {
            let (v, p) = crate::decode_u32_probe(b, code as u32, s, e, cnt);
            (v, p, code != 0x18)
        } else {
            let (v, p) = crate::decode_u16_probe(b, code as u32, s, e, cnt);
            (v, p, code != 0x18)
        };
        if vals.len() != cnt {
            return Gate::fail(format!(
                "{name}: LoadHeader: stream {} (code {code:#04x}) decoded {} values, device requires {cnt}",
                i, vals.len()
            ));
        }
        if strict && pos != e {
            return Gate::fail(format!(
                "{name}: LoadHeader: stream {} (code {code:#04x}) consumed to {pos:#x}, window ends {e:#x}",
                i,
            ));
        }
        if !strict {
            // Device DecodeSimple9 keeps scanning the window after the vector fills;
            // a bad mode nibble anywhere in the window kills the load.
            let mut p = s;
            while p + 4 <= e {
                let m = u32le(b, p) >> 28;
                if !(1..=9).contains(&m) {
                    return Gate::fail(format!(
                        "{name}: LoadHeader: stream {} Simple9 bad mode {m} at {p:#x}",
                        i,
                    ));
                }
                p += 4;
            }
        }
        match i {
            4 => rel_files = vals.iter().map(|&v| v as u16).collect(),
            5 => cats = vals.iter().map(|&v| v as u16).collect(),
            _ => {}
        }
        streams.push(vals);
    }
    Ok(NlHeader {
        hdr: h,
        blob_end,
        elem,
        nb,
        nrec,
        nrel,
        shift,
        origin,
        streams,
        rel_files,
        cats,
    })
}

// ---------------------------------------------------------------------------
// Device-faithful NLAsfBlock::SetDataBlock (00cdf404) gate — reverse engineered
// 2026-09-25 from DAPIAPP.OUT:
//   enSetListDescriptions 00cdc0c0  (TOC -> NLPSFListDescriptor objects; never fails)
//   enDecodeTreeStructure  00cdf360  (0x8403/0x403, 0x4402/0x402, 0x406, 0x401)
//   enDecodeAttrLists      00cddf18  (all attr columns, exact abort order)
// Descriptor object = {u32 start, u32 window, u32 count(f4), u16 tag, u16 sub};
// code byte = sub low byte; window_i = start[i+1]-start[i] in TOC row order; the
// LAST row's window start field is read 8 bytes past the TOC (file bytes, i.e.
// garbage by design).  Streams past the container buffer are read by the device
// OUT OF ITS FILE BUFFER (heap) - flagged here as `danger`; values are
// unpredictable on the device, and this is what silently corrupts valid-destination
// bitmaps in single-block generated files.
// ---------------------------------------------------------------------------

/// One parsed TOC row.
#[derive(Clone, Copy)]
struct TocRow {
    tag: u16,
    sub: u16,
    start: u32,
    count: u32,
}

/// Object descriptor half: (start rel base, window bytes, count, code).
#[derive(Clone, Copy, Default)]
struct Desc {
    start: u32,
    win: u32,
    count: u32,
    code: u8,
}

/// Device NLStandardDecoder success for numeric columns (u32 or u16 width).
/// Returns (ok, decoded_values_or_empty, cursor_pos).
fn std_decode(b: &[u8], base: usize, d: Desc, n: usize, wide: bool) -> (bool, Vec<u32>, usize) {
    let s = base + d.start as usize;
    let e = base + d.start as usize + d.win as usize;
    let e = e.min(b.len()); // clamp; overrun reported separately
    let (v, p) = if wide {
        crate::decode_u32_probe(b, d.code as u32, s, e, n)
    } else {
        crate::decode_u16_probe(b, d.code as u32, s, e, n)
    };
    let ok = match d.code {
        0x11 | 0x14 | 0x16 | 0x18 => v.len() == n && p == e,
        0x12 | 0x13 | 0x15 | 0x17 => true,
        _ => false,
    };
    (ok, v, p)
}

/// Device NLBitfieldDecoder: codes 1/2/3 never fail (they stop at window end);
/// any other code = hard failure.  Returns (ok, bits).
fn bitfield_decode(b: &[u8], base: usize, d: Desc, n: usize) -> (bool, Vec<bool>) {
    if !matches!(d.code, 1 | 2 | 3) {
        return (false, vec![]);
    }
    let (bits, _) = crate::bitfield_probe(
        b,
        d.code as u32,
        base + d.start as usize,
        base + d.start as usize + d.win as usize,
        n,
    );
    (true, bits)
}

fn overrun(b: &[u8], base: usize, d: Desc) -> bool {
    // Every device decoder loop is bounded by its WINDOW end (ptr < end), so a stream can
    // only touch [start, start+win) relative to the block buffer. start == block_len with
    // win 0 (the stock zero-read cluster tail) is NOT a read.
    d.start as u64 + d.win as u64 > (b.len() - base) as u64
}

/// Full SetDataBlock simulation of block `bi`.  `danger` collects columns whose
/// streams are read past the container buffer (heap-read on the device).
pub struct BlockLoad {
    pub ne: u16,
    pub root_edges: Vec<(String, u32)>, // root node edge labels + interval end
    pub letters: String,                // chars bGetNextValidCharacters would offer at root
    pub valid_dest_popcount: usize,     // bits in col 0x40d (bIsEntryValidDestination)
    pub valid_bits: Vec<bool>,          // full 0x40d column (element-indexed)
    pub danger: Vec<String>,
}

pub fn check_block_load(name: &str, b: &[u8], h: &NlHeader, bi: usize) -> Result<BlockLoad, Gate> {
    let f = |m: String| Gate::fail(format!("{name}: SetDataBlock blk{bi}: {m}"));
    let base = if bi == 0 {
        h.blob_end
    } else {
        h.streams[1][bi] as usize
    };
    if base + 8 > b.len() {
        return f("block header outside file".into());
    }
    let (ne1, roots, ne, ndesc) = (
        u16le(b, base) as usize,
        u16le(b, base + 2) as usize,
        u16le(b, base + 4) as usize,
        u16le(b, base + 6) as usize,
    );
    let _ = roots;
    let toc = base + 8;
    let mut rows = Vec::with_capacity(ndesc);
    for i in 0..ndesc {
        let o = toc + i * 12;
        if o + 12 > b.len() {
            return f("TOC outside file".into());
        }
        rows.push(TocRow {
            tag: u16le(b, o),
            sub: u16le(b, o + 2),
            start: u32le(b, o + 4),
            count: u32le(b, o + 8),
        });
    }
    let win_of = |i: usize| -> u32 {
        if i + 1 < rows.len() {
            rows[i + 1].start.wrapping_sub(rows[i].start)
        } else {
            // device reads the next row's start field 8 bytes past the last TOC row
            let p = toc + ndesc * 12 + 8;
            let garbage = if p + 4 <= b.len() {
                u32le(b, p)
            } else {
                u32::MAX
            };
            garbage.wrapping_sub(rows[i].start)
        }
    };
    let mk = |i: usize| Desc {
        start: rows[i].start,
        win: win_of(i),
        count: rows[i].count,
        code: rows[i].sub as u8,
    };

    // ---- stage 1: descriptor objects (tag&0xfff dispatch, nibble routing) ----
    // one Desc-slot per class half; unset slots stay zero (ctor default).
    let mut o_edge = [Desc::default(); 2]; // [main 0x8403, aux 0x403]
    let mut o_link = [Desc::default(); 2]; // [main 0x4402 bitmap, aux 0x402 pairs]
    let mut o_deg = Desc::default(); // 0x401
    let mut o_claim = Desc::default(); // 0x406
    let mut o_404 = [Desc::default(); 3]; // [n4,n8,n0]
    let mut o_405 = [Desc::default(); 3];
    let mut o_pos = [Desc::default(); 2]; // [n4 bitmap, n0 pairs]
    let mut o_sv = [[Desc::default(); 2]; 5]; // 0x408,0x40a,0x40b,0x40c,0x40e
    let mut o_409 = [Desc::default(); 3];
    let mut o_bin = [Desc::default(); 8]; // 0x40d,0x40f,0x410,0x411,0x412,0x413,0x414,0x416
    let mut o_bl = [Desc::default(); 2]; // [main 0x8415, bitmap 0x415]
    let bin_idx = |tag: u16| -> Option<usize> {
        Some(match tag {
            0x40d => 0,
            0x40f => 1,
            0x410 => 2,
            0x411 => 3,
            0x412 => 4,
            0x413 => 5,
            0x414 => 6,
            0x416 => 7,
            _ => return None,
        })
    };
    for (i, r) in rows.iter().enumerate() {
        let d = mk(i);
        let nib = r.tag & 0xf000;
        match r.tag & 0xfff {
            0x403 => o_edge[if nib == 0 { 1 } else { 0 }] = d,
            0x401 => o_deg = d,
            0x402 => o_link[if nib == 0 { 1 } else { 0 }] = d,
            0x404 => {
                if nib == 0x4000 {
                    o_404[0] = d
                } else if nib == 0x8000 {
                    o_404[1] = d
                } else if nib == 0 {
                    o_404[2] = d
                }
            }
            0x405 => {
                if nib == 0x4000 {
                    o_405[0] = d
                } else if nib == 0x8000 {
                    o_405[1] = d
                } else if nib == 0 {
                    o_405[2] = d
                }
            }
            0x406 => o_claim = d,
            0x407 => o_pos[if nib == 0x4000 { 0 } else { 1 }] = d,
            0x408 => o_sv[0][if nib == 0x4000 { 0 } else { 1 }] = d,
            0x40a => o_sv[1][if nib == 0x4000 { 0 } else { 1 }] = d,
            0x40b => o_sv[2][if nib == 0x4000 { 0 } else { 1 }] = d,
            0x40c => o_sv[3][if nib == 0x4000 { 0 } else { 1 }] = d,
            0x40e => o_sv[4][if nib == 0x4000 { 0 } else { 1 }] = d,
            0x409 => {
                if nib == 0x4000 {
                    o_409[0] = d
                } else if nib == 0x8000 {
                    o_409[1] = d
                } else if nib == 0 {
                    o_409[2] = d
                }
            }
            0x415 => o_bl[if nib == 0 { 1 } else { 0 }] = d,
            0x8403 => o_edge[0] = d,
            0x8415 => o_bl[0] = d,
            c => {
                if nib == 0 {
                    if let Some(j) = bin_idx(c) {
                        o_bin[j] = d;
                    }
                }
            }
        }
    }
    let mut danger = Vec::new();
    let mut chk_ovr = |col: &str, d: Desc, danger: &mut Vec<String>| {
        if overrun(b, base, d) {
            danger.push(format!(
                "col {col} start {:#x} win {:#x} reads past block buffer",
                d.start, d.win
            ));
        }
    };

    // ---- stage 2: enDecodeTreeStructure ----
    let (ok, offs, _) = std_decode(b, base, o_edge[0], o_edge[0].count as usize, true);
    if !ok {
        return f(format!(
            "0x8403 edge offsets decode fail (code {:#04x})",
            o_edge[0].code
        ));
    }
    chk_ovr("0x8403", o_edge[0], &mut danger);
    chk_ovr("0x403", o_edge[1], &mut danger); // raw blob copy
    let (ok_bmp, link_bits) = bitfield_decode(b, base, o_link[0], o_deg.count as usize);
    if !ok_bmp {
        return f(format!(
            "0x4402 link bitmap bad code {:#04x}",
            o_link[0].code
        ));
    }
    chk_ovr("0x4402", o_link[0], &mut danger);
    let npairs = link_bits.iter().filter(|x| **x).count() * 2;
    let (ok, _, _) = std_decode(b, base, o_link[1], npairs, true);
    if !ok {
        return f(format!(
            "0x402 link pairs decode fail (code {:#04x}, n={npairs})",
            o_link[1].code
        ));
    }
    chk_ovr("0x402", o_link[1], &mut danger);
    let (ok, claims, _) = std_decode(b, base, o_claim, o_claim.count as usize, true);
    if !ok {
        return f(format!(
            "0x406 claims decode fail (code {:#04x})",
            o_claim.code
        ));
    }
    chk_ovr("0x406", o_claim, &mut danger);
    let (ok, degs, _) = std_decode(b, base, o_deg, ne1, false);
    if !ok {
        return f(format!(
            "0x401 degrees decode fail (code {:#04x})",
            o_deg.code
        ));
    }
    chk_ovr("0x401", o_deg, &mut danger);

    // ---- stage 3: enDecodeAttrLists (exact device order, opts all =1) ----
    let valuelist = |o: [Desc; 3],
                     nbitmap: usize,
                     wide: bool,
                     col: &str,
                     danger: &mut Vec<String>|
     -> Result<(), Gate> {
        let (ok, _) = bitfield_decode(b, base, o[0], nbitmap);
        if !ok {
            return Err(Gate(format!(
                "col {col} rank bitmap bad code {:#04x}",
                o[0].code
            )));
        }
        chk_ovr(col, o[0], danger);
        let (okr, _, _) = std_decode(b, base, o[1], o[1].count as usize, wide);
        let _ = okr; // device ignores rank decode result
        chk_ovr(col, o[1], danger);
        let (okv, _, _) = std_decode(b, base, o[2], o[2].count as usize, wide);
        if !okv {
            return Err(Gate(format!(
                "col {col} values decode fail (code {:#04x})",
                o[2].code
            )));
        }
        chk_ovr(col, o[2], danger);
        Ok(())
    };
    valuelist(o_404, ne1, false, "0x404", &mut danger)?;
    valuelist(o_405, ne1, false, "0x405", &mut danger)?;
    let single = |o: [Desc; 2], col: &str, danger: &mut Vec<String>| -> Result<usize, Gate> {
        let (ok, bits) = bitfield_decode(b, base, o[0], o[0].count as usize);
        if !ok {
            return Err(Gate(format!(
                "col {col} bitmap bad code {:#04x}",
                o[0].code
            )));
        }
        chk_ovr(col, o[0], danger);
        let pc = bits.iter().filter(|x| **x).count();
        if pc != 0 {
            let (okv, _, _) = std_decode(b, base, o[1], pc, true);
            if !okv {
                return Err(Gate(format!(
                    "col {col} values decode fail (code {:#04x})",
                    o[1].code
                )));
            }
            chk_ovr(col, o[1], danger);
        }
        Ok(pc)
    };
    // bitmap-only binary columns in exact order: 0x413, 0x40f, 0x40e(sv), 0x410, 0x40d,
    // 0x411, 0x412, 0x414, 0x416(ignore). Position + 0x409 + singles interleaved per device.
    let binary =
        |d: Desc, col: &str, danger: &mut Vec<String>| -> Result<(usize, Vec<bool>), Gate> {
            if d.count != 0 {
                let (ok, bits) = bitfield_decode(b, base, d, d.count as usize);
                if !ok {
                    return Err(Gate(format!("col {col} bitmap bad code {:#04x}", d.code)));
                }
                let pc = bits.iter().filter(|x| **x).count();
                chk_ovr(col, d, danger);
                Ok((pc, bits))
            } else {
                Ok((0, Vec::new()))
            }
        };
    let (pc_413, _) = binary(o_bin[5], "0x413", &mut danger)?;
    // position (opts[0]): bitmap = n4 row; pairs = n0 row; count = 2*popcount
    let (ok, posbits) = bitfield_decode(b, base, o_pos[0], o_pos[0].count as usize);
    if !ok {
        return f(format!("0x4407 pos bitmap bad code {:#04x}", o_pos[0].code));
    }
    chk_ovr("0x4407", o_pos[0], &mut danger);
    let npc = posbits.iter().filter(|x| **x).count() * 2;
    let (ok, _, _) = std_decode(b, base, o_pos[1], npc, true);
    if !ok {
        return f(format!(
            "0x407 pos pairs decode fail (code {:#04x}, n={npc})",
            o_pos[1].code
        ));
    }
    chk_ovr("0x407", o_pos[1], &mut danger);
    single(o_sv[0], "0x408", &mut danger)?; // opts[1]
    valuelist(o_409, ne1, true, "0x409", &mut danger)?; // opts[2]
    single(o_sv[3], "0x40c", &mut danger)?; // opts[3]
    let pc_40b = single(o_sv[2], "0x40b", &mut danger)?; // opts[4]
    single(o_sv[1], "0x40a", &mut danger)?; // opts[5]
    let _ = binary(o_bin[1], "0x40f", &mut danger)?; // opts[8]
    single(o_sv[4], "0x40e", &mut danger)?; // opts[7]
    let _ = binary(o_bin[2], "0x410", &mut danger)?; // opts[9]
    let (pc_40d, bits_40d) = binary(o_bin[0], "0x40d", &mut danger)?; // opts[6] valid destinations
    let _ = binary(o_bin[3], "0x411", &mut danger)?; // opts[10]
    let _ = binary(o_bin[4], "0x412", &mut danger)?; // opts[11]
    let _ = binary(o_bin[6], "0x414", &mut danger)?; // opts[12]
    let _ = pc_413;
    let _ = pc_40b;
    // BinList 0x415/0x8415 (unconditional, aborts): main S9 list + aux bitmap
    if o_bl[0].count != 0 {
        let (ok, _, _) = std_decode(b, base, o_bl[0], o_bl[0].count as usize, true);
        if !ok {
            return f(format!(
                "0x8415 char-status decode fail (code {:#04x})",
                o_bl[0].code
            ));
        }
        chk_ovr("0x8415", o_bl[0], &mut danger);
    }
    let _ = binary(o_bl[1], "0x415", &mut danger)?; // aux bitmap result ignored... (device runs it, result used for 0x490 obj)

    // ---- root NLInputStep (bInitialise 00cf7130) ----
    let nedges = degs.iter().map(|x| *x as usize).sum::<usize>();
    if nedges != claims.len() {
        return f(format!(
            "degrees sum {nedges} != claim count {}",
            claims.len()
        ));
    }
    let mut root_edges = Vec::new();
    let mut letters = String::new();
    let mut acc = 0usize;
    let d0 = degs[0] as usize;
    for e in 0..d0 {
        let off_a = offs[e] as usize;
        let off_b = if e + 1 < d0 {
            offs[e + 1] as usize
        } else {
            o_edge[1].count as usize
        };
        let blob = base + o_edge[1].start as usize;
        if blob + off_b > b.len() {
            return f(format!("root edge {e} label outside file"));
        }
        let label = String::from_utf8_lossy(&b[blob + off_a..blob + off_b]).to_string();
        let end = acc + claims[e] as usize - 1;
        acc += claims[e] as usize;
        if let Some(ch) = label.chars().next() {
            if !letters.contains(ch) {
                letters.push(ch);
            }
        }
        root_edges.push((label, end as u32));
    }
    Ok(BlockLoad {
        ne: ne as u16,
        root_edges,
        letters,
        valid_dest_popcount: pc_40d,
        valid_bits: bits_40d,
        danger,
    })
}

/// Replica of `LISA_tclAddressSearch::vPopulateCityIndices` `00c73b68` candidate collection +
/// keep-gates, given the city name-list, its REL matrices and the RSI context ranges.
/// Device chain (all CONFIRMED 2026-09-25): RSI ranges -> `bUpdateRange` `00c8c23c` ->
/// `NLRelationProcessor::bGetRelationsPerIndex` `00cf1f28` (per-index matrix query, multimap
/// key = queried city element) -> candidate set = keys inside the RSI ranges -> per candidate:
/// `bGoToElement` + `enGetAllElementProperties`==1 && props+8==-1 (ctor default = pass) +
/// current UI lang in the META s6 set (stock META = pass) + `bIsEntryValidDestination` (col
/// 0x40d). Homonym `u32GetCntElemWithSameName>1` / name & position bookkeeping drives display
/// sorting only. Element-existence stands in for bGoToElement (index < element count).
pub struct CityList {
    pub ne: usize,
    pub keys: usize,
    pub candidates: usize,
    pub list: Vec<u32>,
    pub dropped: Vec<(u32, &'static str)>,
    pub danger: Vec<String>,
}

/// `rels` = (rel file id, bytes, by_source) — by_source=true when the city is the matrix SOURCE
/// side (REL00000 2<->2), false when it is the TARGET side (REL00001 3->2, REL00003 10->2,
/// REL00006 9->2); pairs returned keyed by city. `ctx` = RSI ranges (inclusive lo, exclusive hi).
pub fn check_city_list(
    lid: &[u8],
    rels: &[(u16, &[u8], bool)],
    ctx: &[(u32, u32)],
) -> Result<CityList, Gate> {
    use crate::rel::{get_relations, RelIndex};
    let h = check_name_list_header("city.lid", lid)?;
    let mut valid: Vec<bool> = Vec::new();
    let mut danger = Vec::new();
    for bi in 0..h.nb as usize {
        let bl = check_block_load("city.lid", lid, &h, bi)?;
        valid.extend_from_slice(&bl.valid_bits);
        danger.extend(bl.danger);
    }
    let ne = valid.len();
    let mut keys = std::collections::BTreeSet::new();
    for (id, bytes, bys) in rels {
        let idx = RelIndex::parse(bytes).map_err(|e| Gate(format!("REL{id:05}: parse: {e}")))?;
        for &(lo, hi) in ctx {
            for (k, _) in get_relations(bytes, &idx, *bys, lo, hi)
                .map_err(|e| Gate(format!("REL{id:05}: query: {e}")))?
            {
                keys.insert(k);
            }
        }
    }
    let nkeys = keys.len();
    let mut list = Vec::new();
    let mut dropped = Vec::new();
    let mut candidates = 0;
    for k in keys {
        if !ctx.iter().any(|&(lo, hi)| k >= lo && k < hi) {
            continue;
        }
        candidates += 1;
        if (k as usize) >= ne {
            dropped.push((k, "bGoToElement"));
        } else if !valid[k as usize] {
            dropped.push((k, "validDestination"));
        } else {
            list.push(k);
        }
    }
    Ok(CityList {
        ne,
        keys: nkeys,
        candidates,
        list,
        dropped,
        danger,
    })
}

/// Device street-screen fetch (`LISA_tclAddressSearch` REL-row stage): selecting city elements
/// queries REL00001 rows (city = TARGET side) and lists the street elements they name, after
/// the same element gates as the city screen (`bGoToElement` existence + `bIsEntryValidDestination`).
pub fn check_street_list(
    lid: &[u8],
    rel1: &[u8],
    city_of: &[(u32, u32)],
) -> Result<Vec<u32>, Gate> {
    use crate::rel::{get_relations, RelIndex};
    let h = check_name_list_header("street.lid", lid)?;
    let mut valid: Vec<bool> = Vec::new();
    for bi in 0..h.nb as usize {
        let bl = check_block_load("street.lid", lid, &h, bi)?;
        valid.extend_from_slice(&bl.valid_bits);
    }
    let idx = RelIndex::parse(rel1).map_err(|e| Gate(format!("REL00001: {e}")))?;
    let mut out = std::collections::BTreeSet::new();
    for &(lo, hi) in city_of {
        for (_, street) in
            get_relations(rel1, &idx, false, lo, hi).map_err(|e| Gate(format!("REL00001: {e}")))?
        {
            if (street as usize) < valid.len() && valid[street as usize] {
                out.insert(street);
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// One decoded house number record (device `enGetHnr` answer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HnrRec {
    pub street: u32,
    pub number: u32,
    pub even: bool,
}

/// Device HNR read path over a GenAttr (fileID+20000) file: `read_gen_attr` (=
/// `NLGenAttrFile::DecodeSubHeader` `00e0e3d0` incl. TOC tiling check) then per block replay
/// `enGetHnrIndices` `00e0c3a0` (existence bitmap `0xc01/0x4000` + cumulative starts
/// `0xc01/0x8000`) and `enGetHnr` `00e0d078` (gate `0xc11.param`, number `0xc01`, owner
/// `0xc02`, parity `0xc09` even / `0xc0a` odd). Fails the device path on gate overrun,
/// start-vector gaps/overflow, or parity contradiction.
pub fn check_hnr(b: &[u8]) -> Result<Vec<HnrRec>, Gate> {
    if !crate::is_gen_attr(b) {
        return Err(Gate(
            "HNR: not a GenAttr file (TOC does not tile [0,elem))".into(),
        ));
    }
    let gi = crate::read_gen_attr(b).map_err(|e| Gate(format!("HNR: {e}")))?;
    let mut out = Vec::new();
    for bi in 0..gi.blocks.len() {
        let blk = gi
            .decode_block(b, bi)
            .map_err(|e| Gate(format!("HNR blk{bi}: decode: {e}")))?;
        let st = |col: u32, flag: u32| -> Result<&crate::GenAttrStream, Gate> {
            blk.streams
                .iter()
                .find(|s| s.col == col && s.flags == flag)
                .ok_or_else(|| {
                    Gate(format!(
                        "HNR blk{bi}: missing column {col:#05x}/{flag:#06x}"
                    ))
                })
        };
        // enDecodeHnr object map (CONFIRMED 2026-09-26): +0x28 value VL = selector 0xc01 (u32
        // per-record number), +0x260/+0x280 even/odd parity bits = 0xc09/0xc0a (per-record
        // GROUP flags, independent of the number), +0x380 gate VL existence = 0xc11, owner
        // street is implicit: enGetHnrIndices maps an element to [start, next start) records.
        let ex = &st(0xc01, 0x4000)?.bits;
        let starts = &st(0xc01, 0x8000)?.values;
        let nums = &st(0xc01, 0)?.values;
        let gate_bits = &st(0xc11, 0x4000)?.bits;
        let ev = &st(0xc09, 0)?.bits;
        let od = &st(0xc0a, 0)?.bits;
        let gate_n = gate_bits.len().max(st(0xc11, 0)?.values.len());
        if gate_n < nums.len() {
            return Err(Gate(format!(
                "HNR blk{bi}: gate 0xc11 covers {gate_n} < {} records",
                nums.len()
            )));
        }
        if ev.len() < nums.len() || od.len() < nums.len() {
            return Err(Gate(format!(
                "HNR blk{bi}: parity vectors shorter than records"
            )));
        }
        let span = (gi.blocks[bi].elem_end - gi.blocks[bi].elem_start + 1) as usize; // INCLUSIVE
        let existing: Vec<usize> = (0..span.min(ex.len())).filter(|&e| ex[e]).collect();
        if starts.len() != existing.len() {
            return Err(Gate(format!(
                "HNR blk{bi}: {} starts for {} existing elements",
                starts.len(),
                existing.len()
            )));
        }
        if existing.is_empty() != nums.is_empty() {
            return Err(Gate(format!(
                "HNR blk{bi}: records/elements existence mismatch"
            )));
        }
        // record -> owner: the k-th existing element owns records [starts[k], starts[k+1]).
        let owner_of = |rec: usize| -> Result<u32, Gate> {
            for (k, lo) in starts.iter().enumerate() {
                let hi = starts.get(k + 1).copied().unwrap_or(nums.len() as u32) as usize;
                if rec < hi {
                    if rec < *lo as usize {
                        return Err(Gate(format!("HNR blk{bi}: record {rec} below starts")));
                    }
                    return Ok(gi.blocks[bi].elem_start + existing[k] as u32);
                }
            }
            Err(Gate(format!("HNR blk{bi}: record {rec} past last start")))
        };
        for rec in 0..nums.len() {
            out.push(HnrRec {
                street: owner_of(rec)?,
                number: nums[rec],
                even: ev[rec],
            });
        }
    }
    Ok(out)
}
