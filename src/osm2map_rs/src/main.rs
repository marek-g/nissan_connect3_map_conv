// Bosch TravelMap (Nissan LCN2KAI) OSM -> MAP/IDX writer. Emits the DECOMPRESSED
// .IDX/.MAP layout (the same layout map2osm_rs reads), to be compressed with
// cprnav_compress_rs for deployment.
//
// M2 milestone: parse a real OSM XML extract, tile its objects across levels 0-3
// of a fixed region (N6E2), and emit a multi-level .IDX/.MAP that round-trips
// through map2osm_rs. Semantics (feature codes / names) are minimal here; M3 refines.

use quick_xml::events::Event;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;

const SHIFTS: [i32; 4] = [13, 10, 7, 4]; // coordinate delta shift per level (u32a low byte)
const LATPART: [u8; 4] = [1, 5, 10, 10]; // partition bytes +1 and +2
const TILECNT: [usize; 4] = [1, 25, 2500, 250000];

// N6E2 region BBox in PAU (measured from the stock decompressed N6E2AA.IDX).
const W: i64 = 0x0CCCCCCC; // 18.00 deg E
const S: i64 = 0x21999997; // 47.25 deg N
const E: i64 = 0x19999998; // 36.00 deg E
const N: i64 = 0x2851EB82; // 56.70 deg N

const PAU: f64 = (1i64 << 31) as f64 / 180.0;
fn deg2pau(d: f64) -> i64 {
    (d * PAU).round() as i64
}

const STATE: u16 = 0x25D4; // measured from reference N6E2 polygon cells
const PROF: u16 = 0x12; // base32 "0I" -> file N6E210I.MAP

fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}

fn write_cell(b: &mut [u8], w: u32, feat: u16, w3: u16, w4: u16, t0: u16, t1: u16) {
    let o = (w as usize) * 4;
    put_u16(&mut b[o..o + 2], 0, STATE);
    put_u16(&mut b[o + 2..o + 4], 0, feat);
    put_u16(&mut b[o + 4..o + 6], 0, w3);
    put_u16(&mut b[o + 6..o + 8], 0, w4);
    put_u16(&mut b[o + 8..o + 10], 0, t0); // annotDesc low word = start (in words)
    put_u16(&mut b[o + 10..o + 12], 0, t1); // annotDesc high word = count
}

// A line feature (road or waterway). `ann` is the per-feature annotation entry
// (type, u16 payload w, u32 payload d): roads -> (0x11, roadinfo_w, 0),
// waterways -> (0x10, watercode, 0). None = no annotation.
#[derive(Clone)]
struct LineCell {
    pts: Vec<(i64, i64)>,
    feat: u16,
    ann: Option<(u8, u16, u32)>,
}

// Annotation entry size in words (0x11 roadinfo = 8 bytes; everything else = 4).
fn ann_words(typ: u8) -> u32 {
    match typ {
        0x11 => 2,
        _ => 1,
    }
}

// ---- decompressed MAP block (inverted from map2osm parse_block) ------------
fn build_block(
    shift: i32,
    cx: i64,
    cy: i64,
    polys: &[(Vec<(i64, i64)>, u16)], // (open ring, landuse feat)
    lines: &[LineCell],
    pois: &[(i64, i64, u16, Option<&str>)],
) -> Vec<u8> {
    let (np, nl, nq) = (polys.len(), lines.len(), pois.len());
    let start0: u32 = 4;
    let start1 = start0 + (np as u32) * 3;
    let start2 = start1 + (nl as u32) * 3;
    let cells_end = start2 + (nq as u32) * 3;

    // point pool: one word {i16 dlon, i16 dlat} per vertex, after all cells.
    let mut pp: Vec<u8> = Vec::new();
    let mut poly_idx = vec![0u32; np];
    let mut line_idx = vec![0u32; nl];
    let mut cur = cells_end;
    for (i, (pts, _)) in polys.iter().enumerate() {
        poly_idx[i] = cur;
        for &(lo, la) in pts {
            pp.extend_from_slice(&(((lo - cx) >> shift) as i16).to_le_bytes());
            pp.extend_from_slice(&(((la - cy) >> shift) as i16).to_le_bytes());
            cur += 1;
        }
    }
    for (i, lc) in lines.iter().enumerate() {
        line_idx[i] = cur;
        for &(lo, la) in &lc.pts {
            pp.extend_from_slice(&(((lo - cx) >> shift) as i16).to_le_bytes());
            pp.extend_from_slice(&(((la - cy) >> shift) as i16).to_le_bytes());
            cur += 1;
        }
    }
    let pool_end = cur;

    // annotation region (after point pool): one variable-size entry per line that has an
    // ann, then one 0x7A text entry per named POI, then the POI text records.
    let mut line_ann = vec![None::<u32>; nl];
    let mut w = pool_end;
    for i in 0..nl {
        if let Some((typ, _, _)) = lines[i].ann {
            line_ann[i] = Some(w);
            w += ann_words(typ);
        }
    }
    let mut ann_word = vec![0u32; nq];
    for i in 0..nq {
        if pois[i].3.map_or(false, |s| !s.is_empty()) {
            ann_word[i] = w;
            w += 1; // one word per POI annotation entry
        }
    }
    let mut tw = w;
    let mut text_word = vec![0u32; nq];
    for i in 0..nq {
        if let Some(s) = pois[i].3 {
            if !s.is_empty() {
                text_word[i] = tw;
                tw += ((s.len() + 4 + 3) / 4) as u32; // record = L+4 bytes, word-aligned
            }
        }
    }
    let total_words = tw;

    let mut b = vec![0u8; (total_words as usize) * 4];
    b[0..4].copy_from_slice(&(((0xFFFFu32 << 16) | (total_words & 0xFFFF)).to_le_bytes()));
    put_u16(&mut b, 4, start0 as u16);
    put_u16(&mut b, 6, np as u16);
    put_u16(&mut b, 8, start1 as u16);
    put_u16(&mut b, 10, nl as u16);
    put_u16(&mut b, 12, start2 as u16);
    put_u16(&mut b, 14, nq as u16);

    let mut cw = start0;
    for (i, (rpts, feat)) in polys.iter().enumerate() {
        write_cell(&mut b, cw, *feat, poly_idx[i] as u16, rpts.len() as u16, 0, 0);
        cw += 3;
    }
    for (i, lc) in lines.iter().enumerate() {
        let (t0, t1) = match line_ann[i] {
            Some(aw) => (aw as u16, 1u16),
            None => (0, 0),
        };
        write_cell(&mut b, cw, lc.feat, line_idx[i] as u16, lc.pts.len() as u16, t0, t1);
        cw += 3;
    }
    for i in 0..nq {
        let (lo, la, feat, name) = (pois[i].0, pois[i].1, pois[i].2, pois[i].3);
        let dlon = ((lo - cx) >> shift) as i16 as u16;
        let dlat = ((la - cy) >> shift) as i16 as u16;
        let (t0, t1) = match name {
            Some(s) if !s.is_empty() => (ann_word[i] as u16, 1u16),
            _ => (0, 0),
        };
        write_cell(&mut b, cw, feat, dlon, dlat, t0, t1);
        cw += 3;
    }

    b[(cells_end as usize) * 4..(pool_end as usize) * 4].copy_from_slice(&pp);

    // line annotations (variable size): 0x11 roadinfo {8,0x11,u16 w,u32 d} or
    // 0x10 water {4,0x10,u16 w}.
    for i in 0..nl {
        if let (Some(aw), Some((typ, wval, dval))) = (line_ann[i], lines[i].ann) {
            let bo = (aw as usize) * 4;
            match typ {
                0x11 => {
                    b[bo] = 8;
                    b[bo + 1] = 0x11;
                    put_u16(&mut b, bo + 2, wval);
                    b[bo + 4..bo + 8].copy_from_slice(&dval.to_le_bytes());
                }
                _ => {
                    b[bo] = 4;
                    b[bo + 1] = typ;
                    put_u16(&mut b, bo + 2, wval);
                }
            }
        }
    }

    for i in 0..nq {
        if let Some(s) = pois[i].3 {
            if !s.is_empty() {
                let aw = (ann_word[i] as usize) * 4;
                b[aw] = 4; // size
                b[aw + 1] = 0x7A; // type = TEXT
                put_u16(&mut b, aw + 2, text_word[i] as u16);
                let tp = (text_word[i] as usize) * 4;
                let bytes = s.as_bytes();
                b[tp] = 1; // n strings
                b[tp + 1] = 0;
                b[tp + 2] = bytes.len() as u8;
                b[tp + 3..tp + 3 + bytes.len()].copy_from_slice(bytes);
                b[tp + 3 + bytes.len()] = 0; // terminator
            }
        }
    }
    b
}

// Tile center (PAU) for tile K of `level` — mirrors map2osm_rs tile_extent+tile_box.
fn tile_center(level: usize, k: i64) -> (i64, i64) {
    let w = E - W;
    let h = N - S;
    let (rw, rs, re, rn) = match level {
        0 => (0, 0, w, h),
        1 => {
            let c = k % 5;
            let r = k / 5;
            (w * c / 5, h * r / 5, w * (c + 1) / 5, h * (r + 1) / 5)
        }
        2 => {
            let p = k / 100;
            let t = k % 100;
            let col = (p % 5) * 10 + (t % 10);
            let row = (p / 5) * 10 + (t / 10);
            (w * col / 50, h * row / 50, w * (col + 1) / 50, h * (row + 1) / 50)
        }
        _ => {
            let p = k / 10000;
            let s = (k / 100) % 100;
            let t = k % 100;
            let col = (p % 5) * 100 + (s % 10) * 10 + (t % 10);
            let row = (p / 5) * 100 + (s / 10) * 10 + (t / 10);
            (w * col / 500, h * row / 500, w * (col + 1) / 500, h * (row + 1) / 500)
        }
    };
    let a = SHIFTS[level] as i64 + 1;
    let al = |x: i64| (x >> a) << a;
    let w2 = al(W + rw);
    let s2 = al(S + rs);
    let e2 = al(W + re);
    let n2 = al(S + rn);
    ((w2 + e2) / 2, (s2 + n2) / 2)
}

// ---- shape division across the per-level tile grid --------------------------
// The TravelMap format stores each tile's geometry as i16 deltas from the tile center, so a
// shape can only live in one tile if all its vertices fit that tile's delta range. We therefore
// split every line/polygon along the level's axis-aligned tile grid: each tile receives exactly
// the slice of the shape inside its extent (Liang-Barsky for lines, Sutherland-Hodgman for
// polygons). Slices meet on shared boundary vertices, so the reassembled map is continuous and
// every stored delta stays within i16 range.

fn grid_size(level: usize) -> i64 {
    [1, 5, 50, 500][level]
}

// Aligned rectangular extent (PAU) of cell (col,row) at `level` — mirrors map2osm tile_extent.
fn cell_rect(level: usize, col: i64, row: i64) -> (i64, i64, i64, i64) {
    let w = E - W;
    let h = N - S;
    let G = grid_size(level);
    let rw = w * col / G;
    let rs = h * row / G;
    let re = w * (col + 1) / G;
    let rn = h * (row + 1) / G;
    let a = SHIFTS[level] as i64 + 1;
    let al = |x: i64| (x >> a) << a;
    (al(W + rw), al(S + rs), al(W + re), al(S + rn))
}

// (col,row) -> tile index K, inverse of the level's space-filling mapping.
fn cell_to_k(level: usize, col: i64, row: i64) -> i64 {
    match level {
        0 => 0,
        1 => row * 5 + col,
        2 => {
            let p = 5 * (row / 10) + (col / 10);
            let t = 10 * (row % 10) + (col % 10);
            p * 100 + t
        }
        _ => {
            let p = 5 * (row / 100) + (col / 100);
            let s = 10 * ((row / 10) % 10) + ((col / 10) % 10);
            let t = 10 * (row % 10) + (col % 10);
            p * 10000 + s * 100 + t
        }
    }
}

// Which cell a point falls in at `level` (0..G-1 per axis).
fn point_col_row(level: usize, lon: i64, lat: i64) -> (i64, i64) {
    let w = E - W;
    let h = N - S;
    let G = grid_size(level);
    let fx = ((lon - W) as f64 / w as f64).clamp(0.0, 1.0) * (1.0 - 1e-9);
    let fy = ((lat - S) as f64 / h as f64).clamp(0.0, 1.0) * (1.0 - 1e-9);
    ((fx * G as f64) as i64, (fy * G as f64) as i64)
}

// The grid cells a shape's bounding box spans, padded by one cell and clamped to the grid. A
// straight segment only ever crosses columns/rows between its endpoints', so this covers every
// cell the shape can touch (the +1 margin guards against boundary rounding).
fn cell_span(level: usize, pts: &[(i64, i64)]) -> (i64, i64, i64, i64) {
    let G = grid_size(level);
    let mut cmin = i64::MAX;
    let mut cmax = i64::MIN;
    let mut rmin = i64::MAX;
    let mut rmax = i64::MIN;
    for &(lo, la) in pts {
        let (c, r) = point_col_row(level, lo, la);
        cmin = cmin.min(c);
        cmax = cmax.max(c);
        rmin = rmin.min(r);
        rmax = rmax.max(r);
    }
    (
        cmin.saturating_sub(1).max(0),
        cmax.saturating_add(1).min(G - 1),
        rmin.saturating_sub(1).max(0),
        rmax.saturating_add(1).min(G - 1),
    )
}

// Liang-Barsky clip of segment a->b to rect; returns the portion inside (both endpoints), or None.
// Slab method: for each axis the visible t-range is [min(crossings), max(crossings)]; intersect
// with [0,1]. A zero-delta axis just requires the start coordinate be within bounds.
fn clip_segment(ax: i64, ay: i64, bx: i64, by: i64, rect: (i64, i64, i64, i64)) -> Option<((i64, i64), (i64, i64))> {
    let (rw, rs, re, rn) = rect;
    let x0 = ax as f64;
    let y0 = ay as f64;
    let dx = bx as f64 - x0;
    let dy = by as f64 - y0;
    let mut t0 = 0.0f64;
    let mut t1 = 1.0f64;
    if dx == 0.0 {
        if x0 < rw as f64 || x0 > re as f64 {
            return None;
        }
    } else {
        let a = (rw as f64 - x0) / dx;
        let b = (re as f64 - x0) / dx;
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    if dy == 0.0 {
        if y0 < rs as f64 || y0 > rn as f64 {
            return None;
        }
    } else {
        let a = (rs as f64 - y0) / dy;
        let b = (rn as f64 - y0) / dy;
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
    }
    if t0 > t1 {
        return None;
    }
    let start = ((x0 + t0 * dx).round() as i64, (y0 + t0 * dy).round() as i64);
    let end = ((x0 + t1 * dx).round() as i64, (y0 + t1 * dy).round() as i64);
    Some((start, end))
}

// Clip a polyline to rect: the portion inside, with boundary intersection points inserted.
fn clip_polyline(pts: &[(i64, i64)], rect: (i64, i64, i64, i64)) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    for i in 0..pts.len().saturating_sub(1) {
        if let Some((p, q)) = clip_segment(pts[i].0, pts[i].1, pts[i + 1].0, pts[i + 1].1, rect) {
            if out.last() != Some(&p) {
                out.push(p);
            }
            if out.last() != Some(&q) {
                out.push(q);
            }
        }
    }
    out
}

// Sutherland-Hodgman clip of a polygon against one half-plane (keep where `inside(axis(pt))`).
fn sh_clip<A, I>(poly: &[(i64, i64)], axis: A, val: i64, inside: I) -> Vec<(i64, i64)>
where
    A: Fn((i64, i64)) -> i64,
    I: Fn(i64) -> bool,
{
    let n = poly.len();
    let mut out = Vec::new();
    if n == 0 {
        return out;
    }
    for i in 0..n {
        let s = poly[i];
        let e = poly[(i + 1) % n];
        let sc = axis(s);
        let ec = axis(e);
        let sin = inside(sc);
        let ein = inside(ec);
        if sin {
            out.push(s);
            if !ein && ec != sc {
                let t = (val as f64 - sc as f64) / (ec as f64 - sc as f64);
                let ix = (s.0 as f64 + t * (e.0 as f64 - s.0 as f64)).round() as i64;
                let iy = (s.1 as f64 + t * (e.1 as f64 - s.1 as f64)).round() as i64;
                out.push((ix, iy));
            }
        } else if ein && ec != sc {
            let t = (val as f64 - sc as f64) / (ec as f64 - sc as f64);
            let ix = (s.0 as f64 + t * (e.0 as f64 - s.0 as f64)).round() as i64;
            let iy = (s.1 as f64 + t * (e.1 as f64 - s.1 as f64)).round() as i64;
            out.push((ix, iy));
        }
    }
    out
}

// Clip a polygon to rect (all four half-planes). Returns the intersection (open loop) or empty.
fn clip_polygon(poly: &[(i64, i64)], rect: (i64, i64, i64, i64)) -> Vec<(i64, i64)> {
    let (rw, rs, re, rn) = rect;
    let mut p = sh_clip(poly, |p| p.0, rw, |c| c >= rw);
    p = sh_clip(&p, |p| p.0, re, |c| c <= re);
    p = sh_clip(&p, |p| p.1, rs, |c| c >= rs);
    p = sh_clip(&p, |p| p.1, rn, |c| c <= rn);
    p
}

// Per-tile geometry after clipping.
#[derive(Default)]
struct TileShapes {
    roads: Vec<(Vec<(i64, i64)>, u16)>,     // (slice, roadinfo_w)
    waterways: Vec<(Vec<(i64, i64)>, u16)>, // (slice, watercode)
    areas: Vec<(Vec<(i64, i64)>, u16)>,     // (clipped ring, landuse feat)
    pois: Vec<(i64, i64, u16, String)>,     // (lon, lat, feat, name)
}

// Split every shape across the level's tile grid. POIs go to their point's cell; lines and
// polygons are clipped so each cell holds only the slice inside its extent.
fn distribute(
    level: usize,
    roads: &[(Vec<(i64, i64)>, u16)],
    waterways: &[(Vec<(i64, i64)>, u16)],
    areas: &[(Vec<(i64, i64)>, u16)],
    pois: &[(i64, i64, u16, String)],
) -> HashMap<i64, TileShapes> {
    let mut map: HashMap<i64, TileShapes> = HashMap::new();

    for (lo, la, feat, name) in pois {
        let (c, r) = point_col_row(level, *lo, *la);
        map.entry(cell_to_k(level, c, r))
            .or_default()
            .pois
            .push((*lo, *la, *feat, name.clone()));
    }

    for (geom, w) in roads {
        let (c0, c1, r0, r1) = cell_span(level, geom);
        for c in c0..=c1 {
            for r in r0..=r1 {
                let rect = cell_rect(level, c, r);
                let slice = clip_polyline(geom, rect);
                if slice.len() >= 2 {
                    map.entry(cell_to_k(level, c, r))
                        .or_default()
                        .roads
                        .push((slice, *w));
                }
            }
        }
    }

    for (geom, wc) in waterways {
        let (c0, c1, r0, r1) = cell_span(level, geom);
        for c in c0..=c1 {
            for r in r0..=r1 {
                let rect = cell_rect(level, c, r);
                let slice = clip_polyline(geom, rect);
                if slice.len() >= 2 {
                    map.entry(cell_to_k(level, c, r))
                        .or_default()
                        .waterways
                        .push((slice, *wc));
                }
            }
        }
    }

    for (geom, feat) in areas {
        let (c0, c1, r0, r1) = cell_span(level, geom);
        for c in c0..=c1 {
            for r in r0..=r1 {
                let rect = cell_rect(level, c, r);
                let slice = clip_polygon(geom, rect);
                if slice.len() >= 3 {
                    map.entry(cell_to_k(level, c, r))
                        .or_default()
                        .areas
                        .push((slice, *feat));
                }
            }
        }
    }

    map
}

// ---- OSM parsing -----------------------------------------------------------
fn attr<'i>(e: &'i quick_xml::events::BytesStart<'i>, key: &str) -> Option<&'i str> {
    use std::borrow::Cow;
    e.attributes()
        .filter_map(|a| a.ok())
        .find(|a| a.key.as_ref() == key)
        .and_then(|a| match a.value {
            Cow::Borrowed(s) => Some(s),
            Cow::Owned(_) => None, // OSM values always borrow from the input buffer
        })
}

fn node_coords(e: &quick_xml::events::BytesStart<'_>) -> Option<(i64, i64, i64)> {
    let id = attr(e, "id")?;
    let la = attr(e, "lat")?;
    let lo = attr(e, "lon")?;
    let id: i64 = id.parse().ok()?;
    let la: f64 = la.parse().ok()?;
    let lo: f64 = lo.parse().ok()?;
    Some((id, deg2pau(lo), deg2pau(la)))
}

fn poi_feat(tags: &HashMap<String, String>) -> u16 {
    let g = |k: &str| tags.get(k).map(|s| s.as_str());
    if let Some(a) = g("amenity") {
        match a {
            "parking" => return 0x02,
            "charging_station" | "charging" => return 0x03,
            "fuel" => return 0x04,
            "restaurant" | "fast_food" | "cafe" | "food_court" => return 0x06,
            "car_rental" => return 0x09,
            "school" | "college" | "university" => return 0x10,
            "bar" | "pub" | "beer_house" => return 0x11,
            "pharmacy" => return 0x13,
            "bank" | "atm" => return 0x15,
            "place_of_worship" | "church" | "temple" | "mosque" | "synagogue" => return 0x16,
            _ => {}
        }
    }
    if let Some(t) = g("tourism") {
        match t {
            "hotel" | "guest_house" | "hostel" => return 0x05,
            "attraction" | "museum" | "viewpoint" | "artwork" | "theme_park" => return 0x17,
            _ => {}
        }
    }
    if let Some(s) = g("shop") {
        match s {
            "supermarket" | "convenience" | "greengrocer" | "bakery" | "butcher" | "mall" => {
                return 0x14
            }
            "car" => return 0x07,
            _ => {}
        }
    }
    if let Some(l) = g("leisure") {
        match l {
            "sports_centre" | "stadium" | "pitch" | "golf_course" => return 0x12,
            _ => {}
        }
    }
    if g("railway").map(|r| r == "station").unwrap_or(false) {
        return 0x22;
    }
    if g("office").map(|o| !o.is_empty()).unwrap_or(false) {
        return 0x08;
    }
    if tags.contains_key("place") {
        return 0x21; // settlement (refined in M4)
    }
    0x01
}

// OSM highway -> TravelMap roadinfo `w` payload (the 0x11 annotation). Bit layout is
// Ghidra-confirmed: bits 0-2 netclass (0=motorway..7=service); 4-5 toll; 6-7 ferry;
// 8-9 closed; 12-15 road type (1=long ramp, 2=roundabout, 3=parallel, 9=interconnect/link).
// `w & 7` (netclass) also drives per-level selection. Link roads take road_type=9 so the
// renderer emits <class>_link; roundabouts take road_type=2.
fn roadinfo_w(hw: &str, junction: Option<&str>, toll: bool) -> u16 {
    let is_link = hw.ends_with("_link");
    let base = hw.strip_suffix("_link").unwrap_or(hw);
    let nc = match base {
        "motorway" => 0,
        "trunk" => 1,
        "primary" => 2,
        "secondary" => 3,
        "tertiary" => 4,
        "unclassified" | "road" => 5,
        "residential" | "living_street" => 6,
        _ => 7, // service, track, path, ...
    };
    let mut w = (nc as u16) & 0b111;
    if is_link {
        w |= 9 << 12; // interconnect/link -> <class>_link
    } else if junction == Some("roundabout") {
        w |= 2 << 12; // roundabout
    }
    if toll {
        w |= 0x10; // toll bits 4-5 (decodes to "3" = toll)
    }
    w
}

// OSM area tags -> TravelMap landuse/natural feature code (polygon low byte). None = not a
// recognized area. Inverted from map2osm landuse_osm.
fn area_feat(tags: &HashMap<String, String>) -> Option<u16> {
    let g = |k: &str| tags.get(k).map(|s| s.as_str());
    if let Some(n) = g("natural") {
        match n {
            "water" => return Some(0x48),
            "wood" | "forest" => return Some(0x2B),
            _ => {}
        }
    }
    if let Some(l) = g("landuse") {
        match l {
            "residential" => return Some(0x9C),
            "grass" | "meadow" => return Some(0x38),
            "forest" | "wood" => return Some(0x2B),
            "cemetery" => return Some(0x39),
            "commercial" => return Some(0x3A),
            "water" | "basin" | "reservoir" => return Some(0x48),
            _ => {}
        }
    }
    None
}

// OSM waterway value -> 0x10 payload u16 (high nibble = type, low nibble = class). Type codes
// inverted from map2osm add_semantic: 1=river, 2=canal, 3=stream, 4=ditch.
fn watercode(ww: &str) -> u16 {
    let typ = match ww {
        "river" => 1,
        "canal" => 2,
        "stream" | "brook" | "creek" => 3,
        "ditch" | "drain" => 4,
        _ => 0,
    };
    typ << 4 // class = 0
}

// POI importance: lower = more important = shown at coarser zoom.
fn poi_rank(tags: &HashMap<String, String>) -> u8 {
    let g = |k: &str| tags.get(k).map(|s| s.as_str());
    if let Some(p) = g("place") {
        return match p {
            "city" => 0,
            "town" => 1,
            "village" | "suburb" => 2,
            _ => 3, // hamlet, isolated_dwelling, ...
        };
    }
    // major services worth showing at regional zoom
    if g("amenity").map(|a| matches!(a, "fuel" | "hospital")).unwrap_or(false)
        || g("tourism")
            .map(|t| matches!(t, "hotel" | "attraction" | "museum"))
            .unwrap_or(false)
        || g("shop").map(|s| s == "supermarket").unwrap_or(false)
    {
        return 2;
    }
    3
}

// Max netclass shown at each populated level (higher threshold = more detail).
const MAX_ROAD_NC: [u8; 4] = [2, 4, 7, 7]; // L0 motorway..primary, L1 +sec/tert, L2/L3 all
const MAX_POI_RANK: [u8; 4] = [1, 2, 3, 3]; // L0 cities/towns .. L2/L3 everything

struct OsmData {
    pois: Vec<(i64, i64, u16, String, u8)>,  // (lon_pau, lat_pau, feat, name, rank)
    roads: Vec<(Vec<(i64, i64)>, u16)>,      // (geometry, roadinfo_w)
    waterways: Vec<(Vec<(i64, i64)>, u16)>,  // (geometry, watercode)
    areas: Vec<(Vec<(i64, i64)>, u16)>,      // (open ring, landuse feat)
}

fn parse_osm(path: &str, bw: i64, bs: i64, be: i64, bn: i64) -> OsmData {
    use std::io::BufReader;
    // Stream from disk (not fs::read) so multi-GB extracts don't need a full in-memory buffer.
    let file = fs::File::open(path).expect("open osm");
    let mut reader = quick_xml::Reader::from_reader(BufReader::new(file));
    let mut nodes: HashMap<i64, (i64, i64)> = HashMap::new();
    let mut pois: Vec<(i64, i64, u16, String, u8)> = Vec::new();
    let mut roads: Vec<(Vec<(i64, i64)>, u16)> = Vec::new();
    let mut waterways: Vec<(Vec<(i64, i64)>, u16)> = Vec::new();
    let mut areas: Vec<(Vec<(i64, i64)>, u16)> = Vec::new();

    let mut cur_node: Option<i64> = None;
    let mut cur_tags: HashMap<String, String> = HashMap::new();
    let mut cur_way_ids: Option<Vec<i64>> = None;
    let mut cur_way_tags: HashMap<String, String> = HashMap::new();

    let mut buf = Vec::with_capacity(1024);
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                "node" => {
                    if let Some((id, lo, la)) = node_coords(&e) {
                        nodes.insert(id, (lo, la));
                        cur_node = Some(id);
                        cur_tags.clear();
                    }
                }
                "way" => {
                    cur_way_ids = Some(Vec::new());
                    cur_way_tags.clear();
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "node" => {
                    if let Some((id, lo, la)) = node_coords(&e) {
                        nodes.insert(id, (lo, la));
                    }
                }
                "nd" => {
                    if let Some(ids) = &mut cur_way_ids {
                        if let Some(r) = attr(&e, "ref") {
                            if let Ok(v) = r.parse::<i64>() {
                                ids.push(v);
                            }
                        }
                    }
                }
                "tag" => {
                    if let (Some(k), Some(v)) = (attr(&e, "k"), attr(&e, "v")) {
                        if cur_node.is_some() {
                            cur_tags.insert(k.to_string(), v.to_string());
                        } else if cur_way_ids.is_some() {
                            cur_way_tags.insert(k.to_string(), v.to_string());
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                "node" => {
                    if let Some(nid) = cur_node.take() {
                        let is_poi = cur_tags.keys().any(|k| {
                            matches!(k.as_str(), "amenity" | "tourism" | "shop" | "place")
                        });
                        if is_poi {
                            if let Some((lo, la)) = nodes.get(&nid) {
                                if *lo >= bw && *lo <= be && *la >= bs && *la <= bn {
                                    let name = cur_tags.get("name").cloned().unwrap_or_default();
                                    pois.push((*lo, *la, poi_feat(&cur_tags), name, poi_rank(&cur_tags)));
                                }
                            }
                        }
                        cur_tags.clear();
                    }
                }
                "way" => {
                    if let Some(ids) = cur_way_ids.take() {
                        let tags = std::mem::take(&mut cur_way_tags);
                        let g = |k: &str| tags.get(k).map(|s| s.as_str());
                        let pts: Vec<(i64, i64)> =
                            ids.iter().filter_map(|id| nodes.get(id).copied()).collect();
                        if !pts.is_empty()
                            && pts
                                .iter()
                                .any(|&(lo, la)| lo >= bw && lo <= be && la >= bs && la <= bn)
                        {
                            // Priority: highway > waterway > closed area. Clipping later drops any
                            // part that falls outside the region, so a shape only needs to overlap.
                            if let Some(hw) = g("highway") {
                                if pts.len() >= 2 {
                                    let toll = matches!(g("toll"), Some("yes") | Some("1"));
                                    roads.push((pts, roadinfo_w(hw, g("junction"), toll)));
                                }
                            } else if let Some(ww) = g("waterway") {
                                if pts.len() >= 2 {
                                    waterways.push((pts, watercode(ww)));
                                }
                            } else if ids.len() >= 4 && ids.first() == ids.last() {
                                if let Some(feat) = area_feat(&tags) {
                                    let mut ring = pts;
                                    ring.pop(); // drop the closing vertex -> open loop (Bosch convention)
                                    if ring.len() >= 3 {
                                        areas.push((ring, feat));
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
    OsmData { pois, roads, waterways, areas }
}

// ---- .MAP / .IDX emitters --------------------------------------------------
fn emit_map(path: &Path, data: &[u8], binoff: u16) {
    let filesize = binoff as usize + data.len();
    let mut d = vec![0u8; filesize];
    d[0..2].copy_from_slice(&binoff.to_le_bytes());
    d[2..4].copy_from_slice(&0x34u16.to_le_bytes()); // infoTbl
    d[4..8].copy_from_slice(&(filesize as u32).to_le_bytes()); // fileSize
    d[8..12].copy_from_slice(&(W as u32).to_le_bytes());
    d[12..16].copy_from_slice(&(S as u32).to_le_bytes());
    d[16..20].copy_from_slice(&(E as u32).to_le_bytes());
    d[20..24].copy_from_slice(&(N as u32).to_le_bytes());
    d[24..26].copy_from_slice(&8u16.to_le_bytes()); // @0x18
    d[26..28].copy_from_slice(&4u16.to_le_bytes()); // @0x1a
    d[30..32].copy_from_slice(&(0x8400u16 | PROF).to_le_bytes()); // @0x1e
    d[binoff as usize..].copy_from_slice(data);
    fs::write(path, &d).expect("write MAP");
}

// ---- sub-block packing -----------------------------------------------------
// A block's length is stored in u16 (max 65535 words / 262KB). Dense tiles exceed that, so a
// tile's features are packed into several sub-blocks and the tile slot becomes a multi-entry
// (bit14) referencing each. Real N6E2 data does exactly this (max single block ~63726 words).
const MAX_BLOCK_WORDS: u32 = 0xF800; // per-sub-block cap, below the u16 limit with margin

fn poi_cost(name: &str) -> u32 {
    if name.is_empty() {
        3
    } else {
        3 + 1 + ((name.len() + 4 + 3) / 4) as u32 // cell + text ann word + record
    }
}

// Partition a tile's features into sub-blocks (each <= MAX_BLOCK_WORDS) and build them.
fn pack_and_build_blocks(
    shift: i32,
    cx: i64,
    cy: i64,
    polys: &[(Vec<(i64, i64)>, u16)],
    lines: &[LineCell],
    pois: &[(i64, i64, u16, Option<&str>)],
) -> Vec<Vec<u8>> {
    let np = polys.len();
    let nl = lines.len();
    let n = np + nl + pois.len();
    if n == 0 {
        return Vec::new();
    }
    // word cost per feature (3 cell words + point-pool words + annotation words)
    let mut cost: Vec<u32> = Vec::with_capacity(n);
    for (pts, _) in polys {
        cost.push(3 + pts.len() as u32);
    }
    for lc in lines {
        let aw = match lc.ann {
            Some((t, _, _)) => ann_words(t),
            None => 0,
        };
        cost.push(3 + lc.pts.len() as u32 + aw);
    }
    for p in pois {
        cost.push(poi_cost(p.3.unwrap_or("")));
    }
    // greedy first-fit into blocks (each starts with the 4-word header)
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut used: u32 = 4;
    for i in 0..n {
        if !cur.is_empty() && used + cost[i] > MAX_BLOCK_WORDS {
            groups.push(std::mem::take(&mut cur));
            used = 4;
        }
        cur.push(i);
        used += cost[i];
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    let mut out: Vec<Vec<u8>> = Vec::with_capacity(groups.len());
    for g in &groups {
        let mut gp = Vec::new();
        let mut gl = Vec::new();
        let mut gq = Vec::new();
        for &ri in g {
            if ri < np {
                gp.push(polys[ri].clone());
            } else if ri < np + nl {
                gl.push(lines[ri - np].clone());
            } else {
                gq.push(pois[ri - np - nl]);
            }
        }
        out.push(build_block(shift, cx, cy, &gp, &gl, &gq));
    }
    out
}

// ---- .IDX emitter (single + multi-entry slots) ------------------------------
fn emit_idx(path: &Path, slots: &[Vec<Option<Vec<(u16, u32)>>>; 4]) {
    let partOff: u16 = 16; // partition table @ 0x40
    let mut off = 0x70u32; // first tile table after the partition table
    let mut tbl_offs = [0u32; 4];
    for i in 0..4 {
        tbl_offs[i] = off;
        off += (TILECNT[i] as u32) * 8;
    }
    let fixed_end = off; // end of the four tile tables
    let binOff: u16 = tbl_offs[0] as u16;

    // Assign byte offsets for each multi-tile's sub-entry table (appended after the tables).
    let mut sub_off = fixed_end;
    let mut sub_table: HashMap<(usize, usize), u32> = HashMap::new();
    for L in 0..4usize {
        for k in 0..TILECNT[L] {
            if let Some(e) = &slots[L][k] {
                if e.len() > 1 {
                    sub_table.insert((L, k), sub_off);
                    sub_off += (e.len() * 8) as u32;
                }
            }
        }
    }
    let total = sub_off as usize;
    let mut d = vec![0u8; total];
    d[0..2].copy_from_slice(&binOff.to_le_bytes());
    d[2..4].copy_from_slice(&32u16.to_le_bytes()); // spare
    d[4..8].copy_from_slice(&(W as u32).to_le_bytes());
    d[8..12].copy_from_slice(&(S as u32).to_le_bytes());
    d[12..16].copy_from_slice(&(E as u32).to_le_bytes());
    d[16..20].copy_from_slice(&(N as u32).to_le_bytes());
    d[0x14..0x16].copy_from_slice(&partOff.to_le_bytes());

    for i in 0..4usize {
        let o = 0x40 + i * 12;
        d[o] = i as u8;
        d[o + 1] = LATPART[i];
        d[o + 2] = LATPART[i];
        let u32a = (TILECNT[i] as u32) << 8 | SHIFTS[i] as u32;
        d[o + 3..o + 7].copy_from_slice(&u32a.to_le_bytes());
        d[o + 7..o + 11].copy_from_slice(&(tbl_offs[i] << 8).to_le_bytes());
    }

    let regprof = (0x400u16 | PROF) as u32; // single/sub-entry profile word (bits 14-15 clear)
    for L in 0..4usize {
        let tbl = tbl_offs[L] as usize;
        for k in 0..TILECNT[L] {
            let so = tbl + k * 8;
            match &slots[L][k] {
                None => {
                    d[so..so + 2]
                        .copy_from_slice(&((0x8000u16 | regprof as u16).to_le_bytes())); // empty
                }
                Some(e) if e.len() == 1 => {
                    let (len, offb) = e[0];
                    d[so..so + 2].copy_from_slice(&(regprof as u16).to_le_bytes());
                    d[so + 2..so + 4].copy_from_slice(&len.to_le_bytes());
                    d[so + 4..so + 8].copy_from_slice(&offb.to_le_bytes());
                }
                Some(e) => {
                    let sbo = sub_table[&(L, k)];
                    let a: u32 = ((e.len() as u32) << 16) | (0x4000u16 | PROF) as u32; // bit14 = multi
                    d[so..so + 4].copy_from_slice(&a.to_le_bytes());
                    d[so + 4..so + 8].copy_from_slice(&sbo.to_le_bytes());
                    for (j, &(len, offb)) in e.iter().enumerate() {
                        let q = (sbo as usize) + j * 8;
                        let aa: u32 = ((len as u32) << 16) | regprof;
                        d[q..q + 4].copy_from_slice(&aa.to_le_bytes());
                        d[q + 4..q + 8].copy_from_slice(&offb.to_le_bytes());
                    }
                }
            }
        }
    }
    fs::write(path, &d).expect("write IDX");
}

// ---- TCI (TILE_CLUSTER_INDEX) emission -------------------------------------
// Per-MAP-file sub-index. Layout reverse-engineered from DAPIAPP.OUT (dap_map_tclTCIHeader /
// TCIPartition / u16LoadPartitionTable / u16LoadClusterIndexTile) and confirmed against the
// stock N6E2 10I/11A .TCI files:
//   [0x00] header (20B): u16 f0=0, u16 f1=92, u32 filesize, u16 partOff=0x84, u16 partCnt=4,
//                        u16 f5=122, u16 f6=16, u16 f7=12, u16 f8=106  (format constants)
//   [0x14] descriptive block (112B): copyright/version/"TILE_CLUSTER_INDEX"/"TPNAV2" metadata
//   [0x84] partition table: 4 x {u32 level, u32 tileCount, u32 sectionOffset}
//   [0xb4] per-level tile records: tileCount x {u16 primCl, u16 cl, u32 clusterOffset}
// The runtime (dap_map_tclIdController::u16GenerateTileIds) resolves a region's tiles via the
// .IDX path; the TCI is only consulted for "cluster" tiles and, if the file is absent, logs
// 0x307 "Could not read tci file" and skips them. For profile 10I the stock TCI's cluster pool
// is entirely empty (no geometry), so a structurally-valid all-empty TCI matches what Bosch
// ships: every tile record is zeroed and no cluster data follows.
fn emit_tci(path: &Path, tilecnt: &[usize; 4]) {
    const DESCRIPTIVE_BLOCK: [u8; 112] = [
        0x43, 0x6f, 0x70, 0x79, 0x72, 0x69, 0x67, 0x68, 0x74, 0x20, 0x52, 0x6f, // "Copyright Ro"
        0x62, 0x65, 0x72, 0x74, 0x2d, 0x42, 0x6f, 0x73, 0x63, 0x68, 0x2d, 0x47, // "bert-Bosch-G"
        0x6d, 0x62, 0x48, 0x20, 0x20, 0x32, 0x30, 0x30, 0x33, 0x00, 0x31, 0x42, // "mbH  2003\01B"
        0x39, 0x2e, 0x30, 0x33, 0x2e, 0x31, 0x38, 0x3a, 0x31, 0x33, 0x3a, 0x30, // "9.03.18:13:0"
        0x39, 0x00, 0x54, 0x49, 0x4c, 0x45, 0x5f, 0x43, 0x4c, 0x55, 0x53, 0x54, // "9\0TILE_CLUST"
        0x45, 0x52, 0x5f, 0x49, 0x4e, 0x44, 0x45, 0x58, 0x00, 0x00, 0x00, 0x00, // "ER_INDEX\0\0\0"
        0x14, 0x00, 0x36, 0x00, 0x00, 0x00, 0x46, 0x00, 0x03, 0x00, 0x01, 0x00, // (20,54,0,70,3,1)
        0x00, 0x00, 0x31, 0x42, 0x39, 0x2e, 0x30, 0x33, 0x2e, 0x31, 0x38, 0x3a, // \0\0"1B9.03.18:"
        0x31, 0x31, 0x3a, 0x31, 0x34, 0x00, 0x54, 0x50, 0x4e, 0x41, 0x56, 0x32, // "11:14\0TPNAV2"
        0x00, 0x00, 0x00, 0x00,
    ];
    let prefix = 20 + 112 + 4 * 12; // header + descriptive + partition table = 180
    let rec_total: u32 = tilecnt.iter().map(|&c| c as u32).sum::<u32>() * 8;
    let filesize = (prefix as u32) + rec_total;
    let mut d = vec![0u8; filesize as usize]; // record arrays start zeroed (empty records)

    // header
    d[0..2].copy_from_slice(&0u16.to_le_bytes());
    d[2..4].copy_from_slice(&92u16.to_le_bytes());
    d[4..8].copy_from_slice(&filesize.to_le_bytes());
    d[8..10].copy_from_slice(&0x84u16.to_le_bytes()); // partOff
    d[10..12].copy_from_slice(&4u16.to_le_bytes()); // partCnt
    d[12..14].copy_from_slice(&122u16.to_le_bytes());
    d[14..16].copy_from_slice(&16u16.to_le_bytes());
    d[16..18].copy_from_slice(&12u16.to_le_bytes());
    d[18..20].copy_from_slice(&106u16.to_le_bytes());
    // descriptive block (verbatim stock metadata)
    d[0x14..0x84].copy_from_slice(&DESCRIPTIVE_BLOCK);
    // partition table: sectionOffset[L] = 180 + sum(tilecnt[0..L]) * 8
    let mut off = prefix as u32;
    for (lvl, &cnt) in tilecnt.iter().enumerate() {
        let p = 0x84 + lvl * 12;
        d[p..p + 4].copy_from_slice(&(lvl as u32).to_le_bytes());
        d[p + 4..p + 8].copy_from_slice(&(cnt as u32).to_le_bytes());
        d[p + 8..p + 12].copy_from_slice(&off.to_le_bytes());
        off += cnt as u32 * 8;
    }
    fs::write(path, &d).expect("write TCI");
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let osm_in = args.get(1).cloned().unwrap_or_else(|| {
        "/home/marek/Ext/reverse_engineering/NissanMaps/OSM-map/krzeszowice.osm".into()
    });
    let outdir = args.get(2).cloned().unwrap_or_else(|| "/tmp/opencode/wt2".into());
    fs::create_dir_all(&outdir).ok();

    // Parse bbox in PAU. Optional 3rd arg "W,S,E,N" (degrees); default = full N6E2
    // region bounds (all of Poland) so every object in the file is placed into the
    // region-wide tile grid.
    let (bw, bs, be, bn) = match args.get(3) {
        Some(s) => {
            let mut it = s.split(',').map(|x| deg2pau(x.trim().parse().unwrap()));
            (it.next().unwrap(), it.next().unwrap(), it.next().unwrap(), it.next().unwrap())
        }
        None => (W, S, E, N),
    };

    let t0 = std::time::Instant::now();
    let osm = parse_osm(&osm_in, bw, bs, be, bn);
    eprintln!(
        "parsed {} pois, {} roads, {} waterways, {} areas in bbox ({}s)",
        osm.pois.len(),
        osm.roads.len(),
        osm.waterways.len(),
        osm.areas.len(),
        t0.elapsed().as_secs_f64()
    );

    let map_binoff: u32 = 0x40;
    let mut map_data: Vec<u8> = Vec::new();
    // slots[L][k] = None (empty tile) or Some(list of (lenWords, mapOffset) sub-blocks).
    let mut slots: [Vec<Option<Vec<(u16, u32)>>>; 4] = [
        vec![None; TILECNT[0]],
        vec![None; TILECNT[1]],
        vec![None; TILECNT[2]],
        vec![None; TILECNT[3]],
    ];

    for L in 0..4usize { // populate all four levels (L3 = finest, 500x500 grid)
        let shift = SHIFTS[L];
        let mnc = MAX_ROAD_NC[L];
        let mpr = MAX_POI_RANK[L];

        // Per-level selection: roads by netclass (w & 7), POIs by rank. Waterways and landuse
        // areas are local detail, so they're only emitted at L1/L2 (not the whole-country L0).
        let roads: Vec<(Vec<(i64, i64)>, u16)> = osm
            .roads
            .iter()
            .filter(|(_, w)| (*w & 7) <= mnc as u16)
            .cloned()
            .collect();
        let pois: Vec<(i64, i64, u16, String)> = osm
            .pois
            .iter()
            .filter(|p| p.4 <= mpr)
            .map(|(a, b, c, d, _)| (*a, *b, *c, d.clone()))
            .collect();
        let show_detail = L >= 1;
        let waterways: Vec<(Vec<(i64, i64)>, u16)> = if show_detail {
            osm.waterways.clone()
        } else {
            Vec::new()
        };
        let areas: Vec<(Vec<(i64, i64)>, u16)> = if show_detail {
            osm.areas.clone()
        } else {
            Vec::new()
        };

        // Divide every shape across this level's tile grid (each tile gets its clipped slice).
        let mut dist = distribute(L, &roads, &waterways, &areas, &pois);
        eprintln!(
            "L{}: {} roads(nc<={}) + {} pois(rank<={}) + {} water + {} areas across {} tiles",
            L,
            roads.len(),
            mnc,
            pois.len(),
            mpr,
            waterways.len(),
            areas.len(),
            dist.len()
        );

        let mut keys: Vec<i64> = dist.keys().copied().collect();
        keys.sort();
        let ntiles = keys.len();
        let mut nsub = 0usize;
        for K in keys {
            let ts = dist.remove(&K).unwrap();
            let (cx, cy) = tile_center(L, K);

            // Combine roads (0x11 roadinfo) and waterways (0x10 water) into the line section.
            let mut lines: Vec<LineCell> =
                Vec::with_capacity(ts.roads.len() + ts.waterways.len());
            for (geom, w) in &ts.roads {
                lines.push(LineCell { pts: geom.clone(), feat: 0x30, ann: Some((0x11, *w, 0)) });
            }
            for (geom, wc) in &ts.waterways {
                lines.push(LineCell { pts: geom.clone(), feat: 0x30, ann: Some((0x10, *wc, 0)) });
            }
            let tpois_named: Vec<(i64, i64, u16, Option<&str>)> = ts
                .pois
                .iter()
                .map(|(a, b, c, d)| (*a, *b, *c, Some(d.as_str())))
                .collect();

            // Pack the tile's features into sub-blocks (each <= MAX_BLOCK_WORDS) and append them.
            let subblocks = pack_and_build_blocks(shift, cx, cy, &ts.areas, &lines, &tpois_named);
            nsub += subblocks.len();
            let mut entries: Vec<(u16, u32)> = Vec::with_capacity(subblocks.len());
            for blk in &subblocks {
                let offb = map_binoff + map_data.len() as u32;
                map_data.extend_from_slice(blk);
                entries.push(((blk.len() / 4) as u16, offb));
            }
            slots[L][K as usize] = Some(entries);
        }
        eprintln!("L{}: {} non-empty tiles -> {} sub-blocks", L, ntiles, nsub);
    }

    let map_path = format!("{}/N6E210I.MAP", outdir);
    let idx_path = format!("{}/N6E2AA.IDX", outdir);
    let tci_path = format!("{}/N6E210I.TCI", outdir);
    emit_map(Path::new(&map_path), &map_data, 0x40);
    emit_idx(Path::new(&idx_path), &slots);
    emit_tci(Path::new(&tci_path), &TILECNT);

    eprintln!(
        "wrote {}, {} and {}; map data {} B, {}s total",
        idx_path,
        map_path,
        tci_path,
        map_data.len(),
        t0.elapsed().as_secs_f64()
    );
}
