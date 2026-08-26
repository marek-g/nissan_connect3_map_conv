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

    // tile center (PAU) for tile K of the level
    fn tile_box(&self, level: usize, k: i64) -> (i64, i64) {
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
        let w2 = al(self.west + rel_w);
        let s2 = al(self.south + rel_s);
        let e2 = al(self.west + rel_e);
        let n2 = al(self.south + rel_n);
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

// Decode the payload of a single annotation into OSM tags. Only types whose
// on-disk layout is verified are emitted; raw values are preserved so a future
// OSM->TravelMap writer can reconstruct the bytes exactly (see MAP_IDX_format.md §8).
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
        // water: payload = u16; low nibble = class code, high nibble = type code
        0x10 if size >= 4 && p + 2 <= blk.len() => {
            let v = u16le(blk, p);
            tag!("tm:water_class", (v & 0xF).to_string());
            tag!("tm:water_type", ((v >> 4) & 0xF).to_string());
        }
        // road info: payload = {u16, u32}
        //   u16 bits 0-2 = network class; bit 10 -> flag byte bit0;
        //   bit 3 / bit 11 -> inverted flags; u32 bits 0-3 -> flags;
        //   "intersection-free" (userdef road class) = u32 bit 10.
        0x11 if size >= 8 && p + 6 <= blk.len() => {
            let w = u16le(blk, p);
            let d = u32le(blk, p + 2);
            tag!("tm:netclass", (w & 7).to_string());
            tag!("tm:xfree", ((d >> 10) & 1).to_string());
            tag!("tm:roadinfo", format!("{:04x}:{:08x}", w, d));
        }
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
        _ => {}
    }
    out
}

fn parse_annotations(blk: &[u8], desc: u32) -> (Option<Vec<String>>, Option<String>, Cats, Vec<Tag>) {
    let start = (desc & 0xFFFF) as usize;
    let count = ((desc >> 16) & 0xFFFF) as usize;
    if count == 0 || count > 32 {
        return (None, None, Cats::default(), Vec::new());
    }
    let mut pos = start * 4;
    let mut names = None;
    let mut ref_ = None;
    let mut cats = Cats::default();
    let mut tags = Vec::new();
    for _ in 0..count {
        if pos + 2 > blk.len() {
            break;
        }
        let size = blk[pos] as usize;
        let typ = blk[pos + 1];
        if size < 3 || size > 64 {
            break;
        }
        set_cat(&mut cats, typ);
        tags.extend(ann_tags(blk, pos, size, typ));
        if (typ == 0x7A || typ == 0x14) && pos + 4 <= blk.len() {
            let v = u16le(blk, pos + 2) as usize;
            if let Some(rec) = read_text_record(blk, v * 4) {
                if typ == 0x7A {
                    names = Some(rec);
                } else {
                    ref_ = rec.into_iter().next();
                }
            }
        }
        pos += size;
    }
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
    Way { pts: Vec<(i64, i64)>, tags: Vec<Tag> },
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
                    feats.push(Feature::Way { pts, tags });
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
        Feature::Way { pts, tags } => {
            let keys: Vec<String> = pts.iter().map(|(lo, la)| fmt_key(*lo, *la)).collect();
            if keys.len() > 1 && keys.first() == keys.last() {
                (keys.len() >= 3).then_some(OsmFeat::Way { keys, tags })
            } else {
                (keys.len() >= 2).then_some(OsmFeat::Way { keys, tags })
            }
        }
    }
}

fn write_region_level<W: Write>(w: &mut W, region: &Region, level: usize) -> io::Result<(usize, usize)> {
    let mut maps: HashMap<String, Vec<u8>> = HashMap::new();
    // pass 1: assign node ids (dedup by coordinate) + remember POI tags
    let mut node_ids: HashMap<String, i64> = HashMap::new();
    let mut key_order: Vec<String> = Vec::new();
    let mut poi_tags: HashMap<String, Vec<Tag>> = HashMap::new();
    let mut next_id: i64 = 1;
    for (k, ents, box_) in iter_tiles(region, level) {
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
        eprintln!("usage: map2osm_rs <IDX file or dir> [-r NAME_FILTER] [-l LEVELS] [-o OUTDIR]");
        exit(1);
    }
    let src = &args[0];
    let mut levels = "123".to_string();
    let mut rfilter: Option<String> = None;
    let mut outdir: Option<String> = None;
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
            _ => i += 1,
        }
    }
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
                match fs::File::create(&p).and_then(|f| write_region_level(&mut BufWriter::new(f), &r, level)) {
                    Ok((nn, nw)) => eprintln!("{}: {} nodes, {} ways", p, nn, nw),
                    Err(e) => eprintln!("error: {}: {}", p, e),
                }
            } else {
                let stdout = io::stdout();
                let mut w = BufWriter::new(stdout.lock());
                if let Err(e) = write_region_level(&mut w, &r, level) {
                    eprintln!("error: {}", e);
                }
            }
        }
    }
    eprintln!("done in {:.1}s", t0.elapsed().as_secs_f64());
}
