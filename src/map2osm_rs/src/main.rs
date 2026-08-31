// Bosch TravelMap (Nissan LCN2KAI) MAP -> OSM XML converter (Rust port of map2osm.py).
//
// Output: OSM XML (https://wiki.openstreetmap.org/wiki/OSM_XML), one file per
// region+level, `<REGION>_L<level>.osm`:
//   - POIs        -> <node> with tags
//   - lines       -> open <way>  (>=2 nodes)
//   - polygons    -> closed <way> (first node repeated at the end)
//   - all unique coordinates are deduplicated into single <node> elements,
//     written before the <way>s (no forward references).
//   - every object carries id + version="1" + timestamp (dataset date), as in
//     regular OSM data exports.
//   - tags: `name`, `name:alt` ('; ' separated), `ref` (standard OSM keys) and
//     `tm:kind/tm:layer/tm:tile/tm:profile/tm:state/tm:feature/tm:type`, plus the
//     decoded annotation payloads (see MAP_IDX_format.md §8):
//     `tm:surface`, `tm:elev`, `tm:water_class`/`tm:water_type`,
//     `tm:netclass`/`tm:xfree`/`tm:roadinfo`, `tm:city_display/size/admin/overlap`.
//
// The binary format is identical to the Python converter's docstring; see
// map2osm.py in the parent directory for the full reverse-engineering notes.

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::exit;
use std::time::Instant;

const B32: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
const SHIFTS: [i32; 4] = [13, 10, 7, 4];
// dataset date (EUR 2021.Q1); every object gets version=1 + this timestamp so
// JOSM/osmium treat the file as regular OSM data
const TIMESTAMP: &str = "2021-03-31T00:00:00Z";
const GENERATOR: &str = "map2osm (Bosch TravelMap / Nissan LCN2KAI converter)";

fn u16le(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn i16le(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}
fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

fn prof_suffix(rp: u32) -> String {
    let v = (rp & 0xFF) as usize;
    format!("1{}{}", B32[v / 32] as char, B32[v % 32] as char)
}

fn pau_to_deg(v: i64) -> f64 {
    (v as f64) * 180.0 / (1i64 << 31) as f64
}

// PAU per degree of longitude/latitude (inverse of pau_to_deg); matches rnw2osm_rs.
const PAU: f64 = (1i64 << 31) as f64 / 180.0;

// Bounding box in degrees, same `-b W,S,E,N` syntax as rnw2osm_rs (`none` = disabled).
// Only tiles whose extent shares any area with the box are converted.
struct BBox {
    west: f64,
    south: f64,
    east: f64,
    north: f64,
}

impl BBox {
    // True if this box overlaps an axis-aligned tile extent given as PAU bounds (w,s,e,n).
    fn intersects_pau(&self, w: f64, s: f64, e: f64, n: f64) -> bool {
        self.west * PAU < e && w < self.east * PAU && self.south * PAU < n && s < self.north * PAU
    }
    fn parse(spec: &str) -> Option<BBox> {
        if spec.eq_ignore_ascii_case("none") {
            return None;
        }
        let mut it = spec.split(',');
        let west: f64 = it.next()?.parse().ok()?;
        let south: f64 = it.next()?.parse().ok()?;
        let east: f64 = it.next()?.parse().ok()?;
        let north: f64 = it.next()?.parse().ok()?;
        if it.next().is_some() || !(west < east && south < north) {
            return None;
        }
        Some(BBox { west, south, east, north })
    }
}

#[derive(Serialize, Clone)]
struct Tag {
    #[serde(rename = "@k")]
    k: String,
    #[serde(rename = "@v")]
    v: String,
}

#[derive(Serialize)]
struct Node {
    #[serde(rename = "@id")]
    id: i64,
    #[serde(rename = "@version")]
    version: String,
    #[serde(rename = "@timestamp")]
    timestamp: String,
    #[serde(rename = "@lat")]
    lat: String,
    #[serde(rename = "@lon")]
    lon: String,
    #[serde(rename = "tag", skip_serializing_if = "Vec::is_empty")]
    tags: Vec<Tag>,
}

#[derive(Serialize)]
struct NdRef {
    #[serde(rename = "@ref")]
    reference: i64,
}

#[derive(Serialize)]
struct Way {
    #[serde(rename = "@id")]
    id: i64,
    #[serde(rename = "@version")]
    version: String,
    #[serde(rename = "@timestamp")]
    timestamp: String,
    #[serde(rename = "nd")]
    nds: Vec<NdRef>,
    #[serde(rename = "tag", skip_serializing_if = "Vec::is_empty")]
    tags: Vec<Tag>,
}

#[derive(Serialize)]
#[serde(rename = "osm")]
struct OsmData {
    #[serde(rename = "@version")]
    version: String,
    #[serde(rename = "@generator")]
    generator: String,
    #[serde(rename = "node")]
    nodes: Vec<Node>,
    #[serde(rename = "way")]
    ways: Vec<Way>,
}

struct Region {
    name: String,
    map_dir: String,
    data: Vec<u8>,
    west: i64,
    south: i64,
    east: i64,
    north: i64,
    tables: [(i64, i64); 4], // (tileCnt, tableOffset) per level
}

impl Region {
    fn load(idx_path: &Path) -> io::Result<Region> {
        let data = fs::read(idx_path)?;
        let file_name = idx_path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?
            .to_string_lossy()
            .into_owned();
        let base = &file_name[..file_name.len() - 4]; // strip ".IDX"
        let name = if base.ends_with("AA") {
            &base[..base.len() - 2]
        } else {
            base
        }
        .to_string();
        let map_dir = idx_path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let binoff = u16le(&data, 0) as i64;
        let west = u32le(&data, 4) as i64;
        let south = u32le(&data, 8) as i64;
        let east = u32le(&data, 12) as i64;
        let north = u32le(&data, 16) as i64;
        let pt = (u16le(&data, 0x14) as usize) * 4;
        let mut tables = [(0i64, 0i64); 4];
        for i in 0..4 {
            let o = pt + i * 12;
            let a = u32le(&data, o + 3);
            let b = u32le(&data, o + 7);
            tables[i] = ((a >> 8) as i64, (b >> 8) as i64);
        }
        assert_eq!(tables[0].1, binoff, "L0 table must be at binOff");
        Ok(Region { name, map_dir, data, west, south, east, north, tables })
    }

    // tile slot K -> entries (regProf, lenWords, offset); empty for empty/multi slots
    fn entries(&self, level: usize, k: i64) -> Vec<(u32, u32, u32)> {
        let base = self.tables[level].1 as usize + (k as usize) * 8;
        let d = &self.data;
        let a = u32le(d, base);
        let b = u32le(d, base + 4);
        let lo = a & 0xFFFF;
        if lo & 0x8000 != 0 {
            return Vec::new();
        }
        let mut out = Vec::new();
        if lo & 0x4000 != 0 {
            for j in 0..(a >> 16) {
                let q = b as usize + (j as usize) * 8;
                let aa = u32le(d, q);
                if (aa & 0xFFFF) & 0xC000 == 0 {
                    out.push(((aa & 0x3FFF), (aa >> 16), u32le(d, q + 4)));
                }
            }
        } else {
            out.push(((lo & 0x3FFF) as u32, a >> 16, b));
        }
        out
    }

    // tile extent (PAU) for tile K of the level: (west, south, east, north)
    fn tile_extent(&self, level: usize, k: i64) -> (i64, i64, i64, i64) {
        let w = self.east - self.west;
        let h = self.north - self.south;
        let (rel_w, rel_s, rel_e, rel_n) = match level {
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
        (al(self.west + rel_w), al(self.south + rel_s), al(self.west + rel_e), al(self.south + rel_n))
    }

    // tile center (PAU) for tile K of the level
    fn tile_box(&self, level: usize, k: i64) -> (i64, i64) {
        let (w2, s2, e2, n2) = self.tile_extent(level, k);
        ((w2 + e2) / 2, (s2 + n2) / 2)
    }
}

fn get_map<'a>(maps: &'a mut HashMap<String, Vec<u8>>, path: &str) -> Result<&'a [u8], String> {
    if !maps.contains_key(path) {
        let d = fs::read(path).map_err(|e| e.to_string())?;
        maps.insert(path.to_string(), d);
    }
    Ok(&maps[path])
}

fn read_text_record(blk: &[u8], pos: usize) -> Option<Vec<String>> {
    if pos + 2 > blk.len() {
        return None;
    }
    let n = blk[pos] as usize;
    if (1..=32).contains(&n) {
        let mut q = pos + 1 + 2 * n;
        let mut strs = Vec::new();
        let mut ok = true;
        for i in 0..n {
            let l = blk[pos + 2 + 2 * i] as usize;
            if l == 0 || l > 500 || q + l > blk.len() {
                ok = false;
                break;
            }
            match std::str::from_utf8(&blk[q..q + l]) {
                Ok(t) => strs.push(t.to_string()),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
            q += l;
        }
        if ok && q < blk.len() && blk[q] == 0 {
            return Some(strs);
        }
    }
    let mut q = pos;
    while q < blk.len() && (48..=57).contains(&blk[q]) {
        q += 1;
    }
    if (1..=16).contains(&(q - pos)) && q < blk.len() && blk[q] == 0 {
        return Some(vec![String::from_utf8_lossy(&blk[pos..q]).into_owned()]);
    }
    None
}

#[derive(Default, Clone, Copy)]
struct Cats {
    water: bool,
    roadnum: bool,
    roadinfo: bool,
    city: bool,
    gas: bool,
    parking: bool,
    restaurant: bool,
    rest_area: bool,
    brand: bool,
    landmark: bool,
}

fn set_cat(c: &mut Cats, t: u8) {
    match t {
        0x10 => c.water = true,
        0x11 => c.roadinfo = true,
        0x14 => c.roadnum = true,
        0x21 => c.city = true,
        0x22 => c.rest_area = true,
        0x23 => c.parking = true,
        0x30 => c.gas = true,
        0x34 => c.restaurant = true,
        0x35 => c.brand = true,
        0x52 => c.landmark = true,
        _ => {}
    }
}

// Annotation types that appear as runs of consecutive same-type 4-byte {u16}
// elements ("list"/specification). Grouped into a single ';'-joined tag.
const LIST_SPEC_TYPES: [u8; 12] =
    [0x31, 0x32, 0x33, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49];

fn is_list_spec(t: u8) -> bool {
    LIST_SPEC_TYPES.contains(&t)
}

fn hex_bytes(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{:02x}", x));
    }
    s
}

// Decode the payload of a single (non-list) annotation into OSM tags. Verified
// layouts are decoded to named tm:* tags; anything else falls back to its raw
// payload hex so an OSM->TravelMap writer can reconstruct the bytes exactly
// (see MAP_format.md §8). Raw values are the ground truth.
fn ann_tags(blk: &[u8], pos: usize, size: usize, typ: u8) -> Vec<Tag> {
    let mut out = Vec::new();
    let p = pos + 2; // payload starts after {u8 size, u8 type}
    macro_rules! tag {
        ($k:expr, $v:expr) => {
            out.push(Tag { k: $k.to_string(), v: $v });
        };
    }
    match typ {
        // road surface cover: payload = u16 (enConvertSurface maps it to an internal enum)
        0x01 if size >= 4 && p + 2 <= blk.len() => {
            tag!("tm:surface", u16le(blk, p).to_string());
        }
        // elevation: payload = s8 (relative elevation, pass-through)
        0x03 if size >= 3 && p + 1 <= blk.len() => {
            tag!("tm:elev", (blk[p] as i8).to_string());
        }
        // DCM (3D / city model): payload = {u16, u8, u8, u8, u8}; byte2 = class
        // (u8ConvertDCMClass: 0x00->1, 0x20..0x32 -> 2..20). Raw kept for round-trip.
        0x04 if size >= 8 && p + 6 <= blk.len() => {
            let a = u16le(blk, p);
            let c = blk[p + 3];
            tag!(
                "tm:dcm",
                format!(
                    "{:04x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    a,
                    blk[p + 2],
                    c,
                    blk[p + 4],
                    blk[p + 5]
                )
            );
            let cls = if c == 0 {
                1u32
            } else if (0x20..=0x32).contains(&c) {
                (c - 0x20 + 2) as u32
            } else {
                0
            };
            if cls != 0 {
                tag!("tm:dcm_class", cls.to_string());
            }
        }
        // water: payload = u16; low nibble = class code, high nibble = type code
        0x10 if size >= 4 && p + 2 <= blk.len() => {
            let v = u16le(blk, p);
            tag!("tm:water_class", (v & 0xF).to_string());
            tag!("tm:water_type", ((v >> 4) & 0xF).to_string());
        }
        // road info: payload = {u16 w, u32 d}. Bit layout confirmed via u16ReadRoadInfo +
        // u16ConvertRoadSubAttribs / u8RoadInfo2Flags / u16ConvertRoadClass (Ghidra):
        //   w bits 0-2 = network class; bits 4-5 = toll; bits 6-7 = ferry; bits 8-9 = closed
        //   (DtClose); bits 12-15 = road type (1=long ramp, 2=roundabout, 3=parallel,
        //   9=interconnect/link); bit 10/11 + d bits -> flags (restricted/blocked/tunnel/…).
        //   d bits 4-7 = display class; bit 10 = intersection-free.
        0x11 if size >= 8 && p + 6 <= blk.len() => {
            let w = u16le(blk, p);
            let d = u32le(blk, p + 2);
            tag!("tm:netclass", (w & 7).to_string());
            tag!("tm:xfree", ((d >> 10) & 1).to_string());
            // sub-attributes (u16ConvertRoadSubAttribs), firmware-decoded values:
            let rt = ((w >> 12) & 0xF) as u32;
            tag!("tm:road_type", if rt <= 9 { rt.to_string() } else { "0".to_string() });
            tag!(
                "tm:toll",
                match w & 0x30 {
                    0x10 => "3",
                    0x20 => "2",
                    0x30 => "1",
                    _ => "0",
                }
                .to_string()
            );
            tag!(
                "tm:ferry",
                match w & 0xC0 {
                    0x40 => "3",
                    0x80 => "2",
                    0xC0 => "1",
                    _ => "0",
                }
                .to_string()
            );
            tag!("tm:closed", ((w >> 8) & 0x3).to_string());
            tag!("tm:roadinfo", format!("{:04x}:{:08x}", w, d));
        }
        // note: 0x14 (road number) is handled in parse_annotations (it may repeat on one
        // way and is joined into single ref / tm:roadnum_* tags there).
        // city type: payload = u16
        //   bits 0-3 display level (1..14), bits 4-7 size class (inverted scale),
        //   bits 8-10 admin level (0..14 -> 1..15), bit 15 name-overlapping flag.
        0x21 if size >= 4 && p + 2 <= blk.len() => {
            let v = u16le(blk, p);
            tag!("tm:city_display", (v & 0xF).to_string());
            tag!("tm:city_size", ((v >> 4) & 0xF).to_string());
            tag!("tm:city_admin", ((v >> 8) & 7).to_string());
            tag!("tm:city_overlap", ((v >> 15) & 1).to_string());
        }
        // rest area: payload = u8 pass-through
        0x22 if size >= 3 && p + 1 <= blk.len() => {
            tag!("tm:rest_area", blk[p].to_string());
        }
        // Lossless fallback: preserve the raw payload bytes of any other type
        // (0x23 parking, 0x30 fuel, 0x34 restaurant, 0x35 brand, 0x51 image id,
        // 0x52 landmarks, ...). No semantic assumption -> nothing is dropped.
        // 0x7A (name) is excluded: its payload is a text index already resolved
        // into `name`, so echoing it as raw hex would only be noise.
        _ if typ != 0x7A && size >= 3 && p + (size - 2) <= blk.len() => {
            tag!(format!("tm:raw:{:02x}", typ), hex_bytes(&blk[p..p + size - 2]));
        }
        _ => {}
    }
    out
}

// Join values with ';', dropping consecutive repeats (a way can carry several road
// numbers -> ref="60;A53"; identical mid/status codes collapse to one).
fn join_dedup(items: &[String]) -> String {
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        if out.last().map(|l| l == it) != Some(true) {
            out.push(it.clone());
        }
    }
    out.join(";")
}

fn parse_annotations(blk: &[u8], desc: u32) -> (Option<Vec<String>>, Option<String>, Cats, Vec<Tag>) {
    let start = (desc & 0xFFFF) as usize;
    let count = ((desc >> 16) & 0xFFFF) as usize;
    // Polygon cells can carry a large number of annotations (e.g. many DCM
    // entries). The per-iteration bounds checks below stop the walk at the end
    // of the block or on a malformed size, so a generous cap is safe.
    if count == 0 || count > 4096 {
        return (None, None, Cats::default(), Vec::new());
    }
    let mut pos = start * 4;
    let mut names = None;
    let mut refs: Vec<String> = Vec::new();
    let mut rn_mid: Vec<String> = Vec::new();
    let mut rn_status: Vec<String> = Vec::new();
    let mut cats = Cats::default();
    let mut tags = Vec::new();
    let mut i = 0usize;
    while i < count {
        if pos + 2 > blk.len() {
            break;
        }
        let size = blk[pos] as usize;
        let typ = blk[pos + 1];
        if size < 3 || size > 64 {
            break;
        }
        set_cat(&mut cats, typ);
        if is_list_spec(typ) && size == 4 && pos + 4 <= blk.len() {
            // Group the run of consecutive same-type elements into one tag.
            let mut vals = Vec::new();
            loop {
                vals.push(u16le(blk, pos + 2).to_string());
                pos += size;
                i += 1;
                if i < count && pos + 2 <= blk.len() && blk[pos] == 4 && blk[pos + 1] == typ {
                    continue;
                }
                break;
            }
            tags.push(Tag { k: format!("tm:spec:{:02x}", typ), v: vals.join(";") });
        } else if typ == 0x14 && size >= 8 && pos + 8 <= blk.len() {
            // road number: payload = {u16 textRef, u16 mid, u16 status} @ pos+2. A way can
            // carry several numbers; collect each (resolved text + raw mid/status) and join
            // them into single tags below so no key is duplicated.
            let textref = u16le(blk, pos + 2);
            rn_mid.push(u16le(blk, pos + 4).to_string());
            rn_status.push(u16le(blk, pos + 6).to_string());
            if let Some(rec) = read_text_record(blk, (textref as usize) * 4) {
                if let Some(t) = rec.into_iter().next() {
                    refs.push(t);
                }
            }
            pos += size;
            i += 1;
        } else {
            tags.extend(ann_tags(blk, pos, size, typ));
            if typ == 0x7A && pos + 4 <= blk.len() {
                let v = u16le(blk, pos + 2) as usize;
                if let Some(rec) = read_text_record(blk, v * 4) {
                    names = Some(rec);
                }
            }
            pos += size;
            i += 1;
        }
    }
    // Join multiple road numbers into one tag each (no duplicate keys).
    if !rn_mid.is_empty() {
        tags.push(Tag { k: "tm:roadnum_status".to_string(), v: join_dedup(&rn_status) });
        tags.push(Tag { k: "tm:roadnum_mid".to_string(), v: join_dedup(&rn_mid) });
    }
    let ref_ = if refs.is_empty() { None } else { Some(join_dedup(&refs)) };
    (names, ref_, cats, tags)
}

fn layer_name(kind: &str, c: &Cats) -> String {
    if kind == "poi" {
        let mut extra = Vec::new();
        if c.city {
            extra.push("city");
        }
        if c.gas {
            extra.push("gas");
        }
        if c.parking {
            extra.push("parking");
        }
        if c.restaurant {
            extra.push("restaurant");
        }
        if c.rest_area {
            extra.push("rest_area");
        }
        if c.brand {
            extra.push("brand");
        }
        if c.landmark {
            extra.push("landmark");
        }
        if extra.is_empty() {
            "poi".to_string()
        } else {
            format!("poi:{}", extra.join("+"))
        }
    } else if kind == "line" {
        if c.roadnum || c.roadinfo {
            "road".to_string()
        } else if c.water {
            "water".to_string()
        } else {
            "line".to_string()
        }
    } else if c.water {
        "water_area".to_string()
    } else {
        "area".to_string()
    }
}

fn feature_type(kind: &str, feat: u32) -> i64 {
    let t = (feat & 0xFF) as u8;
    match kind {
        "poi" => match t {
            1..=9 => t as i64,
            0x10..=0x17 => (0xA + (t - 0x10)) as i64,
            0x20..=0x25 => (0x12 + (t - 0x20)) as i64,
            0x26 => 0x1B,
            0x27 => 0x1C,
            _ => 0,
        },
        "line" => match t {
            0x07 => 100,
            0x10..=0x17 => 1,
            0x20 => 3,
            0x21 => 2,
            0x30..=0x37 => 4,
            0x71..=0x73 => 100,
            _ => 0,
        },
        _ => match t {
            0x07 | 0x0F => 100,
            0x10..=0x17 => 1,
            0x18..=0x1F => 0x20,
            0x20..=0x24 => 3,
            0x28..=0x2C => 0x22,
            0x30..=0x32 => 2,
            0x38..=0x3A => 0x21,
            0x40 => 5,
            0x41 => 6,
            0x48 => 0x24,
            0x49 => 0x25,
            0x50..=0x57 => 0x65,
            0x58..=0x5F => 0x33,
            0x60..=0x64 => 0x67,
            0x68..=0x6C => 0x35,
            0x70..=0x72 => 0x66,
            0x78..=0x7A => 0x34,
            0x80 => 0x67,
            0x88 => 0x35,
            0x94 => 4,
            0x9C => 0x23,
            _ => 0,
        },
    }
}

enum Feature {
    Poi { lon: i64, lat: i64, tags: Vec<Tag> },
    Way { pts: Vec<(i64, i64)>, tags: Vec<Tag>, closed: bool },
}

fn make_tags(
    kind: &str,
    state: i64,
    feat: u32,
    k: i64,
    rp: u32,
    names: &Option<Vec<String>>,
    ref_: &Option<String>,
    cats: &Cats,
) -> Vec<Tag> {
    let mut tags = vec![
        Tag { k: "tm:kind".to_string(), v: kind.to_string() },
        Tag { k: "tm:layer".to_string(), v: layer_name(kind, cats) },
        Tag { k: "tm:tile".to_string(), v: k.to_string() },
        Tag { k: "tm:profile".to_string(), v: rp.to_string() },
        Tag { k: "tm:state".to_string(), v: state.to_string() },
        Tag { k: "tm:feature".to_string(), v: feat.to_string() },
        Tag { k: "tm:type".to_string(), v: feature_type(kind, feat).to_string() },
    ];
    if let Some(names) = names {
        if !names.is_empty() {
            tags.push(Tag { k: "name".to_string(), v: names[0].clone() });
            let alts: Vec<&str> = names[1..]
                .iter()
                .filter(|s| *s != &names[0])
                .map(|s| s.as_str())
                .collect();
            if !alts.is_empty() {
                tags.push(Tag { k: "name:alt".to_string(), v: alts.join("; ") });
            }
        }
    }
    if let Some(r) = ref_ {
        tags.push(Tag { k: "ref".to_string(), v: r.clone() });
    }
    tags
}

// Map the decoded tm:* values / categories onto standard OSM keys so the file is
// usable in JOSM/routing. The raw tm:* tags are kept as ground truth; these are
// best-effort semantic overlays. Only mappings with a defensible basis are emitted
// (POI amenities, place for named settlements, waterway/natural for water, highway
// from network class). surface/oneway/link are deliberately NOT mapped: the raw
// values are preserved in tm:* but their exact OSM semantics aren't confirmed.
fn tag_value<'a>(tags: &'a [Tag], k: &str) -> Option<&'a str> {
    tags.iter().find(|t| t.k == k).map(|t| t.v.as_str())
}

fn push_once(tags: &mut Vec<Tag>, k: &str, v: String) {
    if !tags.iter().any(|t| t.k == k) {
        tags.push(Tag { k: k.to_string(), v });
    }
}

// POI feature code (low byte) -> primary OSM category tag(s). Verified against object names
// in a Kraków L3 extract and the official icon taxonomy in POI_MAPPING.DAT (see MAP_format.md
// §7 / §7.1). Only codes confirmed by evidence are mapped; anything else is left to tm:feature.
fn poi_osm(feat: u32) -> Vec<(&'static str, &'static str)> {
    match feat & 0xFF {
        0x02 => vec![("amenity", "parking")],
        0x04 => vec![("amenity", "fuel")],
        0x05 => vec![("tourism", "hotel")],
        0x06 => vec![("amenity", "restaurant")],
        0x07 => vec![("shop", "car")],
        0x08 => vec![("office", "company")],
        0x09 => vec![("amenity", "car_rental")],
        0x10 => vec![("amenity", "school")],
        0x11 => vec![("amenity", "bar")],
        0x12 => vec![("leisure", "sports_centre")],
        0x13 => vec![("amenity", "pharmacy")],
        0x14 => vec![("shop", "supermarket")],
        0x15 => vec![("amenity", "bank")],
        0x16 => vec![("amenity", "place_of_worship")],
        // 0x17 (dec 23) is a mixed leisure/attraction class: sports clubs (SE/SK/TK), castles,
        // beaches, squares — verified from names. tourism=attraction is the OSM catch-all.
        0x17 => vec![("tourism", "attraction")],
        0x22 => vec![("railway", "station")],
        _ => Vec::new(),
    }
}

// Polygon/area feature code (low byte) -> OSM landuse/natural tag(s). Verified against names +
// spatial/size analysis of a Kraków-area L3 extract (§7): 0x2B large forests in the south, 0x48
// water bodies, 0x39 cemeteries, 0x3A shopping/commercial zones, 0x9C small city-centre blocks,
// 0x38 large unnamed rural open land. The unnamed classes (0x9C urban, 0x38 rural) are best-effort;
// the exact code always stays in tm:feature.
fn landuse_osm(feat: u32) -> Vec<(&'static str, &'static str)> {
    match feat & 0xFF {
        0x2B => vec![("natural", "wood"), ("landuse", "forest")],
        0x38 => vec![("landuse", "grass")],
        0x39 => vec![("landuse", "cemetery")],
        0x3A => vec![("landuse", "commercial")],
        0x48 => vec![("natural", "water"), ("water", "lake")],
        0x9C => vec![("landuse", "residential")],
        _ => Vec::new(),
    }
}

fn add_semantic(tags: &mut Vec<Tag>, kind: &str, cats: &Cats) {
    if kind == "poi" {
        // Primary: POI feature code -> OSM category (see poi_osm / §7). Direct and covers more
        // categories than the annotation flags below.
        let feat = tag_value(tags, "tm:feature")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        for (k, v) in poi_osm(feat) {
            push_once(tags, k, v.into());
        }
        // Fallback: categories also detectable via annotation, for POIs whose feature code is
        // not in poi_osm. push_once keeps the feature-based value if it already set the key.
        if !tags.iter().any(|t| t.k == "amenity") {
            if cats.gas {
                push_once(tags, "amenity", "fuel".into());
            } else if cats.parking {
                push_once(tags, "amenity", "parking".into());
            } else if cats.restaurant {
                push_once(tags, "amenity", "restaurant".into());
            }
        }
        // Named settlement -> place (best-effort from size class: 0x1 largest .. 0xC smallest).
        if cats.city {
            let size = tag_value(tags, "tm:city_size").and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
            let place = match size {
                1..=3 => "city",
                4..=6 => "town",
                7..=9 => "village",
                _ => "hamlet",
            };
            push_once(tags, "place", place.into());
        }
    }

    // Rest area -> highway=rest_area (OSM convention for service-area nodes).
    if cats.rest_area {
        push_once(tags, "highway", "rest_area".into());
    }

    // Water: lines -> waterway (best-effort), polygons -> natural=water.
    if cats.water {
        if kind == "line" {
            let wt = tag_value(tags, "tm:water_type").and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);
            let ww = match wt {
                1 => "river",
                2 => "canal",
                3 => "stream",
                4 => "ditch",
                5 => "stream",
                _ => "other",
            };
            push_once(tags, "waterway", ww.into());
        } else if kind == "polygon" {
            // closed water body (lake/reservoir/pond)
            push_once(tags, "natural", "water".into());
            push_once(tags, "water", "lake".into());
        }
    }

    // Areas -> landuse/natural from the feature code (see landuse_osm / §7). Covers water too,
    // so a water polygon without a 0x10 annotation still gets natural=water. Best-effort for the
    // unnamed urban (0x9C) and rural (0x38) classes; push_once keeps any earlier value.
    if kind == "polygon" {
        let feat = tag_value(tags, "tm:feature")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        for (k, v) in landuse_osm(feat) {
            push_once(tags, k, v.into());
        }
    }

    // Roads -> highway from network class (lower class = more major), refined by the
    // Ghidra-confirmed sub-attributes: ferry routes, link roads (long ramp / interconnect),
    // roundabouts, and toll. Raw tm:* values stay as ground truth.
    if kind == "line" && (cats.roadinfo || cats.roadnum) {
        let nc = tag_value(tags, "tm:netclass").and_then(|s| s.parse::<u32>().ok()).unwrap_or(7);
        let base = match nc {
            0 => "motorway",
            1 => "trunk",
            2 => "primary",
            3 => "secondary",
            4 => "tertiary",
            5 => "unclassified",
            6 => "residential",
            _ => "service",
        };
        let rtype = tag_value(tags, "tm:road_type")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let ferry = tag_value(tags, "tm:ferry").and_then(|s| s.parse::<u32>().ok()).unwrap_or(0);

        if ferry != 0 {
            // a ferry route, not a road class
            push_once(tags, "highway", "ferry".into());
        } else if rtype == 1 || rtype == 9 {
            // long ramp / interconnect -> OSM link road: <class>_link (unclassified has no
            // *_link value in OSM -> service_link)
            let link = match base {
                "unclassified" => "service_link".to_string(),
                c => format!("{}_link", c),
            };
            push_once(tags, "highway", link);
        } else {
            push_once(tags, "highway", base.into());
        }
        if rtype == 2 {
            push_once(tags, "junction", "roundabout".into());
        }
        if tag_value(tags, "tm:toll")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0)
            != 0
        {
            push_once(tags, "toll", "yes".into());
        }
    }
}

// (k, entries, (cx, cy)) for every non-empty tile of the level
fn iter_tiles(region: &Region, level: usize) -> Vec<(i64, Vec<(u32, u32, u32)>, (i64, i64))> {
    let cnt = region.tables[level].0;
    let mut out = Vec::new();
    for k in 0..cnt {
        let ents = region.entries(level, k);
        if !ents.is_empty() {
            out.push((k, ents, region.tile_box(level, k)));
        }
    }
    out
}

fn parse_block(
    region: &Region,
    level: usize,
    rp: u32,
    ln: u32,
    off: usize,
    cx: i64,
    cy: i64,
    k: i64,
    maps: &mut HashMap<String, Vec<u8>>,
) -> Result<Vec<Feature>, String> {
    let path = format!("{}/{}{}.MAP", region.map_dir, region.name, prof_suffix(rp));
    let m = get_map(maps, &path)?;
    let mk = u32le(m, off);
    if (mk & 0xFFFF) != ln {
        return Err(format!(
            "{} prof {:04x} @ {:x}: marker {:#010x} != len {}",
            region.name, rp, off, mk, ln
        ));
    }
    let blk = &m[off..off + ln as usize * 4];
    let sh = SHIFTS[level] as i32;
    let mut feats = Vec::new();
    for li in 0..3usize {
        let st = u16le(blk, 4 + li * 4) as usize;
        let cnt = u16le(blk, 6 + li * 4) as usize;
        if cnt == 0 {
            continue;
        }
        let base = st * 4;
        for ci in 0..cnt {
            let p = base + ci * 12;
            if p + 12 > blk.len() {
                break;
            }
            let state = u16le(blk, p) as i64;
            let feat = u16le(blk, p + 2) as u32;
            let w3 = u16le(blk, p + 4);
            let w4 = u16le(blk, p + 6);
            let t0 = u16le(blk, p + 8) as u32;
            let t1 = u16le(blk, p + 10) as u32;
            let (names, ref_, cats, ann) = parse_annotations(blk, (t1 << 16) | t0);
            if li < 2 {
                let pidx = w3 as usize;
                let cnt2 = w4 as usize;
                if pidx * 4 + cnt2 * 4 > blk.len() {
                    continue;
                }
                let mut pts = Vec::with_capacity(cnt2);
                for pi in 0..cnt2 {
                    let dlon = i16le(blk, (pidx + pi) * 4) as i64;
                    let dlat = i16le(blk, (pidx + pi) * 4 + 2) as i64;
                    pts.push((cx + (dlon << sh), cy + (dlat << sh)));
                }
                if !pts.is_empty() {
                    let kind = if li == 0 { "polygon" } else { "line" };
                    let mut tags = make_tags(kind, state, feat, k, rp, &names, &ref_, &cats);
                    tags.extend(ann);
                    add_semantic(&mut tags, kind, &cats);
                    feats.push(Feature::Way { pts, tags, closed: li == 0 });
                }
            } else {
                if feat & 0xF000 == 0xF000 {
                    continue; // premium POI: no plain coordinate
                }
                let dlon = w3 as i16 as i64;
                let dlat = w4 as i16 as i64;
                if dlon == 0 && dlat == 0 {
                    continue;
                }
                let mut tags = make_tags("poi", state, feat, k, rp, &names, &ref_, &cats);
                tags.extend(ann);
                add_semantic(&mut tags, "poi", &cats);
                feats.push(Feature::Poi { lon: cx + (dlon << sh), lat: cy + (dlat << sh), tags });
            }
        }
    }
    Ok(feats)
}

fn fmt_key(lon: i64, lat: i64) -> String {
    format!("{:.8},{:.8}", pau_to_deg(lon), pau_to_deg(lat))
}

enum OsmFeat {
    Node { key: String, tags: Vec<Tag> },
    Way { keys: Vec<String>, tags: Vec<Tag> },
}

fn to_osm(f: Feature) -> Option<OsmFeat> {
    match f {
        Feature::Poi { lon, lat, tags } => Some(OsmFeat::Node { key: fmt_key(lon, lat), tags }),
        Feature::Way { pts, tags, closed } => {
            let mut keys: Vec<String> = pts.iter().map(|(lo, la)| fmt_key(*lo, *la)).collect();
            // Bosch stores polygon vertices once (open loop); OSM areas must be closed rings,
            // so repeat the first vertex at the end.
            if closed && keys.len() >= 3 && keys.first() != keys.last() {
                keys.push(keys[0].clone());
            }
            if closed {
                (keys.len() >= 3).then_some(OsmFeat::Way { keys, tags })
            } else {
                (keys.len() >= 2).then_some(OsmFeat::Way { keys, tags })
            }
        }
    }
}

// Keep a tile only when no bbox is set, or its extent overlaps the requested box.
fn tile_in_bbox(region: &Region, level: usize, k: i64, bbox: &Option<BBox>) -> bool {
    match bbox {
        None => true,
        Some(bb) => {
            let (w, s, e, n) = region.tile_extent(level, k);
            bb.intersects_pau(w as f64, s as f64, e as f64, n as f64)
        }
    }
}

fn write_region_level<W: Write>(w: &mut W, region: &Region, level: usize, bbox: &Option<BBox>) -> io::Result<(usize, usize)> {
    let mut maps: HashMap<String, Vec<u8>> = HashMap::new();
    // pass 1: assign node ids (dedup by coordinate) + remember POI tags
    let mut node_ids: HashMap<String, i64> = HashMap::new();
    let mut key_order: Vec<String> = Vec::new();
    let mut poi_tags: HashMap<String, Vec<Tag>> = HashMap::new();
    let mut next_id: i64 = 1;
    for (k, ents, box_) in iter_tiles(region, level) {
        if !tile_in_bbox(region, level, k, bbox) {
            continue;
        }
        for &(rp, ln, off) in &ents {
            let feats = match parse_block(region, level, rp, ln, off as usize, box_.0, box_.1, k, &mut maps) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("warn: {} L{} tile {}: {}", region.name, level, k, e);
                    continue;
                }
            };
            for f in feats {
                match to_osm(f) {
                    Some(OsmFeat::Node { key, tags }) => {
                        if !node_ids.contains_key(&key) {
                            node_ids.insert(key.clone(), next_id);
                            key_order.push(key.clone());
                            next_id += 1;
                        }
                        poi_tags.insert(key, tags);
                    }
                    Some(OsmFeat::Way { keys, .. }) => {
                        for key in keys {
                            if !node_ids.contains_key(&key) {
                                node_ids.insert(key.clone(), next_id);
                                key_order.push(key.clone());
                                next_id += 1;
                            }
                        }
                    }
                    None => {}
                }
            }
        }
    }

    let mut nodes = Vec::with_capacity(key_order.len());
    for key in &key_order {
        let nid = node_ids[key];
        let (lon_s, lat_s) = key.split_once(',').unwrap();
        let tags = poi_tags.remove(key).unwrap_or_default();
        nodes.push(Node {
            id: nid,
            version: "1".to_string(),
            timestamp: TIMESTAMP.to_string(),
            lat: lat_s.to_string(),
            lon: lon_s.to_string(),
            tags,
        });
    }

    // pass 2: ways
    let mut ways = Vec::new();
    for (k, ents, box_) in iter_tiles(region, level) {
        if !tile_in_bbox(region, level, k, bbox) {
            continue;
        }
        for &(rp, ln, off) in &ents {
            let feats = match parse_block(region, level, rp, ln, off as usize, box_.0, box_.1, k, &mut maps) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("warn: {} L{} tile {}: {}", region.name, level, k, e);
                    continue;
                }
            };
            for f in feats {
                let (keys, tags) = match to_osm(f) {
                    Some(OsmFeat::Way { keys, tags }) => (keys, tags),
                    _ => continue,
                };
                let mut refs: Vec<i64> = Vec::new();
                for key in &keys {
                    let nid = node_ids[key];
                    if refs.last() != Some(&nid) {
                        // drop consecutive duplicates
                        refs.push(nid);
                    }
                }
                if refs.len() < 2 {
                    continue;
                }
                let wid = next_id;
                next_id += 1;
                let nds = refs.into_iter().map(|reference| NdRef { reference }).collect();
                ways.push(Way {
                    id: wid,
                    version: "1".to_string(),
                    timestamp: TIMESTAMP.to_string(),
                    nds,
                    tags,
                });
            }
        }
    }

	let ways_len = ways.len();
	
    let osm = OsmData {
        version: "0.6".to_string(),
        generator: GENERATOR.to_string(),
        nodes,
        ways,
    };

	w.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")?;

	let mut xml_buf = String::new();
    let mut serializer = quick_xml::se::Serializer::new(&mut xml_buf);
    serializer.indent(' ', 4); // 4 spacje wcięcia
    osm.serialize(serializer).map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
	w.write_all(xml_buf.as_bytes())?;

	w.write_all(b"\n")?;

    Ok((key_order.len(), ways_len))
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: map2osm_rs <IDX file or dir> [-r NAME_FILTER] [-l LEVELS] [-b W,S,E,N|none] [-o OUTDIR]");
        exit(1);
    }
    let src = &args[0];
    let mut levels = "123".to_string();
    let mut rfilter: Option<String> = None;
    let mut outdir: Option<String> = None;
    let mut bbox_spec = "none".to_string();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-l" => {
                levels = args[i + 1].clone();
                i += 2;
            }
            "-r" => {
                rfilter = Some(args[i + 1].clone());
                i += 2;
            }
            "-o" => {
                outdir = Some(args[i + 1].clone());
                i += 2;
            }
            "-b" => {
                bbox_spec = args[i + 1].clone();
                i += 2;
            }
            _ => i += 1,
        }
    }
    let bbox = match BBox::parse(&bbox_spec) {
        Some(b) => Some(b),
        None if bbox_spec.eq_ignore_ascii_case("none") => None,
        None => {
            eprintln!("error: invalid -b '{}', expected W,S,E,N (degrees) or 'none'", bbox_spec);
            exit(1);
        }
    };
    let idx_files: Vec<PathBuf> = if Path::new(src).is_dir() {
        let mut v: Vec<PathBuf> = match fs::read_dir(src) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.file_name().map(|n| n.to_string_lossy().ends_with(".IDX")).unwrap_or(false))
                .collect(),
            Err(e) => {
                eprintln!("error: cannot read {}: {}", src, e);
                exit(1);
            }
        };
        v.sort();
        if let Some(f) = &rfilter {
            let wanted: HashSet<String> =
                f.split(',').map(|s| s.to_uppercase()).collect();
            v.retain(|p| {
                let n = p.file_name().unwrap().to_string_lossy().to_string();
                let stem = n.strip_suffix("AA.IDX").unwrap_or(&n).to_uppercase();
                wanted.contains(&stem)
            });
        }
        v
    } else {
        vec![PathBuf::from(src)]
    };
    let lvls: Vec<usize> = levels
        .chars()
        .filter_map(|c| c.to_digit(10))
        .map(|d| d as usize)
        .filter(|&d| d <= 3)
        .collect();
    if let Some(od) = &outdir {
        fs::create_dir_all(od).ok();
    }
    let t0 = Instant::now();
    for ip in &idx_files {
        let r = match Region::load(ip) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error: {}: {}", ip.display(), e);
                continue;
            }
        };
        for &level in &lvls {
            if let Some(od) = &outdir {
                let p = format!("{}/{}_L{}.osm", od, r.name, level);
                match fs::File::create(&p).and_then(|f| write_region_level(&mut BufWriter::new(f), &r, level, &bbox)) {
                    Ok((nn, nw)) => eprintln!("{}: {} nodes, {} ways", p, nn, nw),
                    Err(e) => eprintln!("error: {}: {}", p, e),
                }
            } else {
                let stdout = io::stdout();
                let mut w = BufWriter::new(stdout.lock());
                if let Err(e) = write_region_level(&mut w, &r, level, &bbox) {
                    eprintln!("error: {}", e);
                }
            }
        }
    }
    eprintln!("done in {:.1}s", t0.elapsed().as_secs_f64());
}
