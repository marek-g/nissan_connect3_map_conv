// Bosch TravelMap (Nissan LCN2KAI) RNW (road network) -> OSM XML visualizer.
//
// Converts selected RNW cluster files (`NAV*.DAT`, and/or a directory of them)
// into a single OSM XML file, so the road network can be loaded straight into
// an editor (JOSM / QGIS / Vespucci) for visual inspection.
//
// This is the visualization counterpart of `rnw_extract_rs` (which emits JSONL).
// The cluster/onecell/zerocell parsing is reused verbatim from that tool; only
// the output differs. Road geometry assembly, coordinate system (PAU), and the
// display-class -> highway mapping follow RNW_format.md and rnw_join_rs.
//
// Output objects:
//   - every road (onecell) -> an open <way>; nodes are de-duplicated by exact
//     PAU coordinate so roads that meet at a junction share one <node> and the
//     network renders connected (boundary nodes coincide across clusters too).
//   - with --outlines: each cluster's boundary "outline" polygon (from its DAT
//     header) -> an extra closed <way> tagged rnw:outline=yes.
//
// Tags on road ways: `highway` (derived from the runtime display class, so JOSM
// styles it), `name` / `name:alt`, OSM-standard attributes derived from the onecell
// header (`tunnel` / `bridge` / `junction=roundabout` / `oneway`), the raw RNW class
// fields (`rn_class/rn_netclass/rn_roadtype/rn_link/rn_sec/rn_freeway`), the stored
// `rn_length`, the remaining header flags as `rn_*` (emitted only when set), and source
// keys `rnw_file` / `rnw_cluster` / `rnw_oncell_index` (the onecell's index in its
// cluster — the road's identity, since the format has no separate global road id).
//
// Usage: rnw2osm_rs <NAV*.DAT | dir>... [-o OUT.osm] [--outlines] [-b W,S,E,N|none]

use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::exit;

// ---------------------------------------------------------------------------
// constants + byte readers (shared with rnw_extract_rs)
// ---------------------------------------------------------------------------

const PAU: f64 = (1i64 << 31) as f64 / 180.0;
const BLOCK: usize = 0x4000; // clusters are 16KB-aligned in a NAV file
const TIMESTAMP: &str = "2021-03-31T00:00:00Z"; // dataset vintage (EUR 2021.Q1)
const GENERATOR: &str = "rnw2osm_rs (Bosch RNW -> OSM XML visualizer)";
// Valid geographic range of this dataset (EUR 2021). Used as an origin sanity check to reject
// false-positive cluster headers — random bytes at a 16KB boundary that pass the structural
// checks but whose reference coordinate decodes to a garbage point far outside the real map.
const DS_WEST: f64 = -30.0;
const DS_SOUTH: f64 = 30.0;
const DS_EAST: f64 = 60.0;
const DS_NORTH: f64 = 75.0;

const ORDER: &[(u16, &str)] = &[
    (0, "ann"), (1, "skip"), (2, "ci1"), (3, "ci2"), (4, "zero"),
    (5, "one"), (6, "skip"), (7, "skip"), (8, "pos"), (9, "skip"),
    (10, "skip"),
];

fn u16le(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn i32le(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn u32le(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
fn s8(b: u8) -> i32 {
    if b & 0x80 != 0 { b as i32 - 256 } else { b as i32 }
}
fn s24(b0: u8, b1: u8, b2: u8) -> i32 {
    let v = (b0 as i32) | ((b1 as i32) << 8) | (((b2 as i32) & 0x7F) << 16);
    if b2 & 0x80 != 0 { v - (1 << 24) } else { v }
}

// ---------------------------------------------------------------------------
// geographic sanity filter for the 16KB-aligned cluster scan (as rnw_extract_rs)
// ---------------------------------------------------------------------------

struct BBox {
    west: f64,
    south: f64,
    east: f64,
    north: f64,
}

impl BBox {
    fn contains(&self, lon_pau: f64, lat_pau: f64) -> bool {
        self.west * PAU < lon_pau
            && lon_pau < self.east * PAU
            && self.south * PAU < lat_pau
            && lat_pau < self.north * PAU
    }
    // True if this box overlaps an axis-aligned extent given as PAU bounds
    // (west,south,east,north). Used for the geometric cluster selection: a cluster is
    // converted when its outline footprint shares any area with the requested box.
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

// ---------------------------------------------------------------------------
// OSM XML structures (mirrored from map2osm_rs)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct Tag {
    #[serde(rename = "@k")]
    k: String,
    #[serde(rename = "@v")]
    v: String,
}

fn tag(k: &str, v: impl Into<String>) -> Tag {
    Tag { k: k.to_string(), v: v.into() }
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

// ---------------------------------------------------------------------------
// node id assignment: de-duplicate by exact PAU coordinate so that roads meeting
// at a junction (and duplicate boundary nodes across clusters/files) collapse to
// a single <node>. Node ids are negative, way ids positive -> never collide.
// ---------------------------------------------------------------------------

fn floor_div(a: i64, b: i64) -> i64 {
    let q = a / b;
    if a % b != 0 && (a < 0) != (b < 0) {
        q - 1
    } else {
        q
    }
}

// 1 m of latitude expressed in PAU (1 degree == PAU, 1 degree lat == 111320 m).
const M_TO_PAU: f64 = PAU / 111320.0;

fn parse_snap(s: &str) -> f64 {
    match s.trim().parse::<f64>() {
        Ok(v) if v >= 0.0 => v,
        _ => {
            eprintln!("invalid --snap '{}' (expected meters >= 0)", s);
            exit(1);
        }
    }
}

// How a point may participate in cross-cluster boundary-node merging. The runtime never
// merges by distance — it joins clusters via overlap links (handled separately, Phase C)
// and, as a sparse fallback, a border-marker test on the zerocell (bBordersObjectAtTo/From).
// To emit a connected OSM we must still unify the two copies of each boundary junction that
// every cluster stores; the order below mirrors the app: marker test first, proximity last.
#[derive(Clone, Copy, PartialEq)]
enum NodeKind {
    // Road endpoint whose zerocell carries the border marker (rim/cpx/annot-0x31). Merged
    // with its nearest MARKED twin within `snap` — faithful to the runtime's marker test.
    Marked,
    // Road endpoint without the marker. Merged with a nearby twin within `snap` ONLY when
    // the proximity fallback is enabled (`--no-snap` disables this). OSM-side necessity:
    // these are still the same physical junction stored once per cluster.
    Plain,
    // Non-junction point (outline vertex): exact-match dedup only, never merged.
    Fixed,
}

struct OsmBuilder {
    ids: HashMap<(i64, i64), i64>,
    // spatial hash of snap-eligible node coords (cell -> (x, y, is_marked))
    grid: HashMap<(i64, i64), Vec<(i64, i64, bool)>>,
    next_node: i64, // decrements below 0
    ways: Vec<Way>,
    next_way: i64,  // increments from 1
    snap: i64,       // node-snap radius in PAU (0 = exact match only)
    snap_fallback: bool, // allow Plain (unmarked) nodes to proximity-merge
}

impl OsmBuilder {
    fn new(snap: i64, snap_fallback: bool) -> Self {
        OsmBuilder {
            ids: HashMap::new(),
            grid: HashMap::new(),
            next_node: 0,
            ways: Vec::new(),
            next_way: 1,
            snap,
            snap_fallback,
        }
    }

    // Assign an OSM node id for a PAU coordinate. Exact-match dedup always applies (same
    // coords == same node). Boundary junctions are stored once per cluster a few PAU apart
    // (~0.06-0.08 m), so exact-match alone severs roads at every edge; the near-match merge
    // below unifies them, in the app's order: marker-gated first, proximity fallback last.
    fn id(&mut self, x: i64, y: i64, kind: NodeKind) -> i64 {
        if let Some(&id) = self.ids.get(&(x, y)) {
            return id;
        }
        // Which snap targets to consider (None = no proximity merge for this point):
        //   Marked               -> nearest MARKED twin  (faithful marker test)
        //   Plain (+fallback on) -> nearest ANY twin     (OSM-side necessity)
        //   Plain (no fallback) / Fixed                  -> none
        let mark_only: Option<bool> = match kind {
            NodeKind::Marked => Some(true),
            NodeKind::Plain if self.snap_fallback => Some(false),
            _ => None,
        };
        if self.snap > 0 {
            if let Some(mark_only) = mark_only {
                let (cx, cy) = (floor_div(x, self.snap), floor_div(y, self.snap));
                let r2 = self.snap * self.snap;
                let mut best: Option<(i64, i64)> = None;
                let mut best_d2 = i64::MAX;
                for dx in -1..=1i64 {
                    for dy in -1..=1i64 {
                        if let Some(list) = self.grid.get(&(cx + dx, cy + dy)) {
                            for &(px, py, pm) in list {
                                if mark_only && !pm {
                                    continue;
                                }
                                let ddx = x - px;
                                let ddy = y - py;
                                let d2 = ddx * ddx + ddy * ddy;
                                if d2 <= r2 && d2 < best_d2 {
                                    best_d2 = d2;
                                    best = Some((px, py));
                                }
                            }
                        }
                    }
                }
                if let Some(c) = best {
                    if let Some(&id) = self.ids.get(&c) {
                        return id;
                    }
                }
            }
        }
        self.next_node -= 1;
        let id = self.next_node;
        self.ids.insert((x, y), id);
        // Snap-eligible targets: Marked and Plain (a later twin of either may merge to it).
        // Fixed (outline) points are never merge targets.
        if self.snap > 0 && kind != NodeKind::Fixed {
            let (cx, cy) = (floor_div(x, self.snap), floor_div(y, self.snap));
            self.grid.entry((cx, cy)).or_default().push((x, y, kind == NodeKind::Marked));
        }
        id
    }

    // Append an open way (>=2 distinct nodes). Consecutive duplicate coordinates
    // are collapsed. `border` is aligned with `pts` and gates the snap merge. Returns the
    // (first, last) node ids if a way was emitted.
    fn add_line(
        &mut self,
        pts: &[(i64, i64)],
        border: &[bool],
        tags: Vec<Tag>,
    ) -> Option<(i64, i64)> {
        let mut refs = Vec::with_capacity(pts.len());
        for (i, &(x, y)) in pts.iter().enumerate() {
            let kind = match border.get(i).copied() {
                Some(true) => NodeKind::Marked,
                _ => NodeKind::Plain,
            };
            let id = self.id(x, y, kind);
            if refs.last() != Some(&id) {
                refs.push(id);
            }
        }
        if refs.len() < 2 {
            return None;
        }
        let (f, t) = (refs[0], *refs.last().unwrap());
        self.push_way(refs, tags);
        Some((f, t))
    }

    // Append a closed way (>=3 distinct nodes), repeating the first node at the
    // end. Used for outline polygons. Returns true if emitted.
    fn add_polygon(&mut self, pts: &[(i64, i64)], tags: Vec<Tag>) -> bool {
        let mut refs = Vec::with_capacity(pts.len() + 1);
        for &(x, y) in pts {
            // Outline vertices are not road junctions: exact-match dedup only.
            let id = self.id(x, y, NodeKind::Fixed);
            if refs.last() != Some(&id) {
                refs.push(id);
            }
        }
        if refs.len() < 3 {
            return false;
        }
        if refs.first() != refs.last() {
            let first = refs[0];
            refs.push(first);
        }
        self.push_way(refs, tags);
        true
    }

    fn push_way(&mut self, refs: Vec<i64>, tags: Vec<Tag>) {
        let id = self.next_way;
        self.next_way += 1;
        self.ways.push(Way {
            id,
            version: "1".to_string(),
            timestamp: TIMESTAMP.to_string(),
            nds: refs.into_iter().map(|reference| NdRef { reference }).collect(),
            tags,
        });
    }

    // Rewrite every way's node refs through a union-find root, collapsing
    // consecutive duplicates and dropping ways that degenerate to <2 nodes.
    // Merges cross-cluster boundary nodes that overlap links identify as the same
    // junction but which were stored at slightly different PAU coords (> snap).
    fn apply_node_merges(&mut self, uf: &UnionFind) {
        let mut kept: Vec<Way> = Vec::with_capacity(self.ways.len());
        for mut w in self.ways.drain(..) {
            let mut nr: Vec<i64> = Vec::with_capacity(w.nds.len());
            for nd in &w.nds {
                let r = uf.root(nd.reference);
                if nr.last() != Some(&r) {
                    nr.push(r);
                }
            }
            if nr.len() >= 2 {
                w.nds = nr.into_iter().map(|reference| NdRef { reference }).collect();
                kept.push(w);
            }
        }
        self.ways = kept;
    }

    fn finish(self) -> (Vec<Node>, Vec<Way>) {
        // Emit only nodes still referenced by a surviving way (merged-away nodes
        // are dropped; each root id maps to exactly one canonical coordinate).
        let mut used: HashSet<i64> = HashSet::new();
        for w in &self.ways {
            for nd in &w.nds {
                used.insert(nd.reference);
            }
        }
        let mut id2c: HashMap<i64, (i64, i64)> = HashMap::with_capacity(self.ids.len());
        for (&(x, y), &id) in self.ids.iter() {
            id2c.insert(id, (x, y));
        }
        let mut nodes: Vec<Node> = Vec::with_capacity(used.len());
        for id in used {
            if let Some(&(x, y)) = id2c.get(&id) {
                nodes.push(Node {
                    id,
                    version: "1".to_string(),
                    timestamp: TIMESTAMP.to_string(),
                    lat: format!("{:.7}", y as f64 / PAU),
                    lon: format!("{:.7}", x as f64 / PAU),
                    tags: vec![],
                });
            }
        }
        (nodes, self.ways)
    }
}

// Minimal union-find over OSM node ids for cross-cluster boundary-node merging.
struct UnionFind {
    parent: HashMap<i64, i64>,
}
impl UnionFind {
    fn new() -> Self {
        UnionFind { parent: HashMap::new() }
    }
    // Iterative (no recursion) to avoid stack overflow on long chains.
    fn root(&self, x: i64) -> i64 {
        let mut cur = x;
        loop {
            match self.parent.get(&cur) {
                Some(&p) if p != cur => cur = p,
                _ => return cur,
            }
        }
    }
    fn union(&mut self, a: i64, b: i64) {
        let ra = self.root(a);
        let rb = self.root(b);
        if ra != rb {
            // deterministic root: keep the more-negative id (assigned earlier)
            if rb < ra {
                self.parent.insert(ra, rb);
            } else {
                self.parent.insert(rb, ra);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// display-class -> highway mapping (verbatim from rnw_join_rs; the runtime's own
// rendering hierarchy, enConvertRoadSubattrDisplayClass @0x00888b14)
// ---------------------------------------------------------------------------

fn display_class(rc: i64, nc: i64) -> Option<i64> {
    match rc {
        0 | 1 => match nc {
            0 => Some(2),
            1 => Some(6),
            2 => Some(7),
            3 => Some(8),
            7 => Some(9),
            _ => None,
        },
        2 => Some(9),
        3 => Some(10),
        4 => Some(11),
        5 | 7 => Some(12),
        6 => Some(13),
        _ => None,
    }
}

fn highway_tag(dc: i64, link: bool) -> &'static str {
    let base = match dc {
        2 => "motorway",
        6 => "trunk",
        7 | 8 => "primary",
        9 | 10 => "secondary",
        11 => "tertiary",
        12 => "residential",
        13 => "unclassified",
        _ => return "",
    };
    if link {
        match base {
            "motorway" => return "motorway_link",
            "trunk" => return "trunk_link",
            "primary" => return "primary_link",
            "secondary" => return "secondary_link",
            "tertiary" => return "tertiary_link",
            _ => {}
        }
    }
    base
}

// ---------------------------------------------------------------------------
// RNW cluster parsing (reused from rnw_extract_rs)
// ---------------------------------------------------------------------------

fn read_pts(cd: &[u8], off: usize, cnt: u16, ctype: u8) -> Option<Vec<(i32, i32)>> {
    let ptsize = if ctype == 3 { 4 } else { 6 };
    let mut out = Vec::with_capacity(cnt as usize);
    for i in 0..cnt as usize {
        let q = off + i * ptsize;
        if q + ptsize > cd.len() {
            return None;
        }
        if ctype == 3 {
            out.push((
                i16le(cd, q) as i32,
                i16le(cd, q + 2) as i32,
            ));
        } else {
            out.push((
                s24(cd[q], cd[q + 1], cd[q + 2]),
                s24(cd[q + 3], cd[q + 4], cd[q + 5]),
            ));
        }
    }
    Some(out)
}

fn i16le(d: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([d[o], d[o + 1]])
}

fn read_string(cd: &[u8], off: usize) -> Option<Vec<String>> {
    if off == 0 || off >= cd.len() {
        return None;
    }
    let n = cd[off] as usize;
    if !(1..=16).contains(&n) || off + 1 + 2 * n > cd.len() {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    let mut q = off + 1 + 2 * n;
    for i in 0..n {
        let l = cd[off + 1 + 2 * i + 1] as usize;
        if l == 0 || l > 300 || q + l > cd.len() {
            return None;
        }
        match std::str::from_utf8(&cd[q..q + l]) {
            Ok(s) => out.push(s.to_string()),
            Err(_) => return None,
        }
        q += l;
    }
    Some(out)
}

fn read_names(cd: &[u8], ann_off: usize, ann_cnt: u16) -> Option<Vec<String>> {
    let mut names = None;
    let mut q = ann_off;
    for _ in 0..ann_cnt as usize {
        if q + 4 > cd.len() {
            break;
        }
        let size = u16le(cd, q) as usize;
        let typ = u16le(cd, q + 2);
        if size < 4 || size > 64 {
            break;
        }
        if typ == 0x3C && q + 6 <= cd.len() {
            let toff = u16le(cd, q + 4) as usize;
            if let Some(v) = read_string(cd, toff) {
                names = Some(v);
            }
        }
        q += size;
    }
    names
}

// A zerocell's annotation list is a self-contained nav_tclAnnotList object living at
// `offz`: [u16 data_off][u16 count], then `count` records at data_off, each
// [u16 size][u16 type]... where the real type is (type & 0x7fff) (top bit = flag).
// Returns true if any record has type == `want`. (nav_tclAnnotList::bRead @0x0088fb90.)
fn zerocell_has_annot(cd: &[u8], offz: usize, want: u16) -> bool {
    if offz + 4 > cd.len() {
        return false;
    }
    let data_off = u16le(cd, offz) as usize;
    let count = u16le(cd, offz + 2);
    if data_off == 0 || data_off >= cd.len() {
        return false;
    }
    let mut q = data_off;
    for _ in 0..count as usize {
        if q + 4 > cd.len() {
            break;
        }
        let size = u16le(cd, q) as usize;
        let typ = u16le(cd, q + 2) & 0x7fff;
        if size < 4 {
            break;
        }
        if typ == want {
            return true;
        }
        q += size;
    }
    false
}

// One overlap (cross-cluster continuation) reference on an onecell.
// On-disk LocalOneCellRef = 4 bytes (rnw_tclLocalCellRef::bRead @0x00892494):
//   u16@0: low 10 bits = neighbor onecell index + 1, high 6 bits = direction/side flags
//   u16@2: index into this cluster's ci2 (neighbor-cluster list) + 1
#[derive(Debug)]
struct Overlap {
    cli: u16,   // ci2 index (0-based) of the neighbor cluster
    cell: i32,  // neighbor onecell index (raw), -1 if unset
    #[allow(dead_code)]
    flags: u16, // high-6-bit direction/side flags (retained; closest-pair selects the join)
}

// A neighbor cluster as recorded in the ci2 list. On-disk record = 24 bytes
// [8 ClusterID][14 outline-hdr][2 status]; the comparable cluster number is
// u16@4 of the ID and the origin (lon,lat) is i32@8 / i32@12.
struct Ci2 {
    num: i64,
    lon: i64,
    lat: i64,
}

struct Road {
    pts: Option<Vec<(i64, i64)>>,
    border: Vec<bool>, // per-point: true only for from/to node endpoints (zerocells)
    name: Option<Vec<String>>,
    hdr: u32, // raw onecell header word: class fields + all attribute bits (decoded at emission)
    length: u32, // stored road length (onecell x field, low 24 bits; source's own units)
    overlaps: Vec<Overlap>,
}

struct ClusterData {
    cid: u16,
    origin: (i64, i64),          // cluster's own outline origin (lon,lat) in PAU
    ci2: Vec<Ci2>,               // neighbor clusters (ci2 list) — overlap refs index this
    ci1: Vec<Ci2>,               // neighbor clusters (ci1 list) — down-cell refs index this
    roads: Vec<Option<Road>>,    // indexed by RAW onecell index (None = no shape)
    down: Vec<Vec<Overlap>>,     // per RAW onecell: down-cell (finer-level) refs
    outline: Option<Vec<(i64, i64)>>,
}

fn parse_cluster(
    d: &[u8],
    start: usize,
    end: usize,
    bbox: &Option<BBox>,
    want_fine: bool,
) -> Option<ClusterData> {
    let cd = &d[start..end];
    let cluster_id = u16le(cd, 0);
    // u16@2 is a format word (bit 0x40 selects the outline point encoding below), not a
    // validity bit: fine-detail clusters carry 0x0000 here. Reject only empty ids; the lf
    // pattern + field plausibility + origin sanity further down are the real guards.
    let hdr_flags = u16le(cd, 2);
    if cluster_id == 0 {
        return None;
    }
    // --level 0: coarse tier only. The finer detail is the u16@2==0 cluster tier (the dense
    // residential grid); skip it when the caller wants just the coarser layer.
    if !want_fine && hdr_flags == 0 {
        return None;
    }
    let lon = i32le(cd, 8) as i64;
    let lat = i32le(cd, 12) as i64;
    let shift = cd[0x10] as i8;
    let ooff = u16le(cd, 0x12) as usize;
    let ocnt = u16le(cd, 0x14);
    let lf = u16le(cd, 0x16);
    if lf & 0x30 == 0 {
        return None;
    }
    if !(shift >= 0 && shift <= 12) || ooff >= cd.len() || ocnt > 0x4000 {
        return None;
    }
    let ctype: u8 = if hdr_flags & 0x40 != 0 { 4 } else { 3 };
    let ptsize = if ctype == 4 { 6 } else { 4 };
    if ooff + ptsize * ocnt as usize > cd.len() {
        return None;
    }

    // outline (boundary fence) -> absolute PAU points; not needed for routing but
    // emitted as a polygon under --outlines.
    let outline = read_pts(cd, ooff, ocnt, ctype).map(|raw| {
        raw.into_iter()
            .map(|(dx, dy)| (lon + ((dx as i64) << shift), lat + ((dy as i64) << shift)))
            .collect::<Vec<_>>()
    });

    // Origin sanity (always): a real cluster's reference coordinate lies inside the dataset
    // range; a false-positive header decodes to a garbage origin far outside it. Reject those
    // before the geometric test, whether or not a query box is set.
    let lon_deg = lon as f64 / PAU;
    let lat_deg = lat as f64 / PAU;
    if !(DS_WEST <= lon_deg && lon_deg <= DS_EAST && DS_SOUTH <= lat_deg && lat_deg <= DS_NORTH) {
        return None;
    }

    // Geometric selection: convert the cluster when its outline footprint overlaps the
    // requested box (not merely when its origin point falls inside it). A cluster is a large
    // tile whose roads spill past its origin, so an origin test clips the outer ~0.1° of any
    // area of interest. Fall back to the origin point if no usable outline was decoded.
    if let Some(bb) = bbox {
        let keep = match &outline {
            Some(ol) if !ol.is_empty() => {
                let (w0, s0) = ol[0];
                let (mut w, mut e) = (w0, w0);
                let (mut s, mut n) = (s0, s0);
                for &(x, y) in ol.iter() {
                    if x < w { w = x; }
                    if x > e { e = x; }
                    if y < s { s = y; }
                    if y > n { n = y; }
                }
                bb.intersects_pau(w as f64, s as f64, e as f64, n as f64)
            }
            _ => bb.contains(lon as f64, lat as f64),
        };
        if !keep {
            return None;
        }
    }

    let mut p = ooff + ptsize * ocnt as usize;
    let mut descs: HashMap<&'static str, (usize, u16)> = HashMap::new();
    for &(bit, name) in ORDER {
        if lf & (1 << bit) != 0 {
            if name == "skip" {
                p += 4;
                continue;
            }
            if p + 4 > cd.len() {
                return None;
            }
            descs.insert(name, (u16le(cd, p) as usize, u16le(cd, p + 2)));
            p += 4;
        }
    }
    let Some(&(o_one, oc_)) = descs.get("one") else {
        return None;
    };

    // ci1/ci2 lists -> neighbor clusters (each 24-byte record: [8 ID][14 outline-hdr][2
    // status]). The comparable cluster number is u16@4 of the ID; origin = i32@8 / i32@12.
    // Two separate neighbour tables: overlap refs index ci2 (in-mem +0x64), down-cell refs
    // index ci1 (in-mem +0x5c). Both are ListDesc<nav_tclClusterInfo> of 24-byte records.
    let read_ci = |name: &str| -> Vec<Ci2> {
        let mut v = Vec::new();
        if let Some(&(co, cc)) = descs.get(name) {
            for i in 0..cc as usize {
                let e = co + i * 24;
                if e + 24 > cd.len() {
                    break;
                }
                v.push(Ci2 {
                    num: u16le(cd, e + 4) as i64,
                    lon: i32le(cd, e + 8) as i64,
                    lat: i32le(cd, e + 12) as i64,
                });
            }
        }
        v
    };
    let ci2 = read_ci("ci2");
    let ci1 = read_ci("ci1");

    // position list -> node positions (indexed by zerocell)
    let mut nodes: Vec<(i64, i64)> = Vec::new();
    if let (Some(&(po, pc)), Some(&(_, zc))) = (descs.get("pos"), descs.get("zero")) {
        if pc == zc {
            if let Some(pts) = read_pts(cd, po, pc, ctype) {
                for (dx, dy) in pts {
                    nodes.push((lon + ((dx as i64) << shift), lat + ((dy as i64) << shift)));
                }
            }
        }
    }

    // zerocells -> toNode/fromNode per onecell (bit 15: set = TO, clear = FROM), plus the
    // border marker per node. On-disk u16@0 of the 6-byte record is the in-mem +0x24 flags
    // word (rnw_tclZerocellInternal::bRead @0x00894734): bit 1 = bHasRimAnnotation,
    // bit 4 = bIsPartOfCpxCrossing; a node also qualifies via an annotation of type 0x31 in
    // its annot list. This is the runtime's own "is a border/crossing node" test
    // (bBordersObjectAtTo/From @0x0088c5b4/0x0088c618) — no distance involved.
    let zc_n = descs.get("zero").map(|&(_, c)| c as usize).unwrap_or(0);
    let mut to_node = vec![-1i32; oc_ as usize];
    let mut from_node = vec![-1i32; oc_ as usize];
    let mut node_border = vec![false; zc_n];
    if let Some(&(zo, zc)) = descs.get("zero") {
        for i in 0..zc as usize {
            let qz = zo + i * 6;
            if qz + 6 > cd.len() {
                break;
            }
            let zflags = u16le(cd, qz); // in-mem +0x24 flags word (rim/cpx bits)
            let lzf = u16le(cd, qz + 2);
            let offz = u16le(cd, qz + 4) as usize;
            let rim = zflags & 0x2 != 0; // bHasRimAnnotation (bit 1)
            let cpx = zflags & 0x10 != 0; // bIsPartOfCpxCrossing (bit 4)
            let a31 = lzf & 1 != 0 && zerocell_has_annot(cd, offz, 0x31);
            node_border[i] = rim || cpx || a31;
            let mut q = offz;
            for bit in 0..2u16 {
                if lzf & (1 << bit) == 0 {
                    continue;
                }
                if q + 4 > cd.len() {
                    break;
                }
                let o2 = u16le(cd, q) as usize;
                let c2 = u16le(cd, q + 2);
                q += 4;
                if bit == 1 {
                    for j in 0..c2 as usize {
                        let r = o2 + j * 2;
                        if r + 2 > cd.len() {
                            break;
                        }
                        let v = u16le(cd, r);
                        let oi = (v & 0x3FF) as i32 - 1;
                        if oi >= 0 && oi < oc_ as i32 {
                            if v & 0x8000 != 0 {
                                to_node[oi as usize] = i as i32;
                            } else {
                                from_node[oi as usize] = i as i32;
                            }
                        }
                    }
                }
            }
        }
    }

    // onecells -> roads (shape absolute [type5] or relative-to-toNode [type1])
    // Indexed by RAW onecell index so overlap refs (which name the neighbor's raw
    // onecell) map directly onto this vector. Shapeless onecells stay None.
    let mut roads: Vec<Option<Road>> = (0..oc_ as usize).map(|_| None).collect();
    let mut down_all: Vec<Vec<Overlap>> = (0..oc_ as usize).map(|_| Vec::new()).collect();
    for k in 0..oc_ as usize {
        let p2 = o_one + k * 12;
        if p2 + 12 > cd.len() {
            break;
        }
        let hdr = u32le(cd, p2);
        let oclen = u32le(cd, p2 + 4) & 0x00FF_FFFF; // stored length (u32GetLength)
        let lfo = u16le(cd, p2 + 8);
        let offf = u16le(cd, p2 + 10) as usize;
        if offf == 0 || offf + 4 > cd.len() {
            continue;
        }
        let mut q = offf;
        let (mut ann_off, mut ann_cnt) = (0usize, 0u16);
        let mut shape1: Option<(usize, u16)> = None;
        let mut shape5: Option<(usize, u16)> = None;
        let mut ovl: Option<(usize, u16)> = None;
        let mut downc: Option<(usize, u16)> = None;
        for bit in 0..6u16 {
            if lfo & (1 << bit) == 0 {
                continue;
            }
            if q + 4 > cd.len() {
                break;
            }
            let o2 = u16le(cd, q) as usize;
            let c2 = u16le(cd, q + 2);
            q += 4;
            match bit {
                0 => {
                    ann_off = o2;
                    ann_cnt = c2;
                }
                1 => shape1 = Some((o2, c2)),
                // bit 2 = up-cells (coarser parent refs) — not needed for a top-down export.
                2 => {}
                // bit 3 = down-cells (finer sub-segment refs), same 4-byte encoding as overlaps.
                3 => downc = Some((o2, c2)),
                4 => ovl = Some((o2, c2)),
                5 => shape5 = Some((o2, c2)),
                _ => unreachable!(),
            }
        }
        // overlaps (bit 4) -> cross-cluster continuation refs (4-byte LocalOneCellRef each).
        let mut overlaps: Vec<Overlap> = Vec::new();
        if let Some((oo, oc)) = ovl {
            for j in 0..oc as usize {
                let r2 = oo + j * 4;
                if r2 + 4 > cd.len() {
                    break;
                }
                let w = u16le(cd, r2);
                let cli_stored = u16le(cd, r2 + 2); // ci2 index + 1 (0 = unset)
                if cli_stored == 0 {
                    continue;
                }
                overlaps.push(Overlap {
                    cli: cli_stored - 1,
                    cell: ((w & 0x3FF) as i32) - 1, // neighbor onecell index + 1
                    flags: w >> 10,                 // high 6 bits = direction/side
                });
            }
        }
        // down-cells (bit 3): finer-level sub-segment refs. Identical 4-byte encoding to
        // overlaps ([u16 (iOC|flags), u16 (iClu+1)]); iClu indexes this cluster's ci2 list to a
        // finer neighbour cluster, iOC the onecell there. The runtime breaks a coarse road down
        // into these to reveal the fine grid at high zoom (bCreateDownList @0x0088e140).
        let mut down: Vec<Overlap> = Vec::new();
        if let Some((do_, dc)) = downc {
            for j in 0..dc as usize {
                let r2 = do_ + j * 4;
                if r2 + 4 > cd.len() {
                    break;
                }
                let w = u16le(cd, r2);
                let cli_stored = u16le(cd, r2 + 2);
                if cli_stored == 0 {
                    continue;
                }
                down.push(Overlap {
                    cli: cli_stored - 1,
                    cell: ((w & 0x3FF) as i32) - 1,
                    flags: w >> 10,
                });
            }
        }
        down_all[k] = down;
        // Intermediate shape points: absolute [shape5] or differential-from-fromNode
        // [shape1]. A straight onecell (no inline shape) has none — it is just the
        // segment between its two nodes.
        let mut mid: Vec<(i64, i64)> = Vec::new();
        if let Some((so, sc)) = shape5 {
            if let Some(raw) = read_pts(cd, so, sc, ctype) {
                mid = raw
                    .into_iter()
                    .map(|(dx, dy)| (lon + ((dx as i64) << shift), lat + ((dy as i64) << shift)))
                    .collect();
            }
        } else if let Some((so, sc)) = shape1 {
            if sc >= 1 && so + 2 * sc as usize <= cd.len() {
                let dvec: Vec<(i64, i64)> = (0..sc as usize)
                    .map(|i| {
                        (
                            s8(cd[so + i * 2]) as i64 * 256,
                            s8(cd[so + i * 2 + 1]) as i64 * 256,
                        )
                    })
                    .collect();
                // Ghidra-confirmed (vCoordReduced2Absolute @0x00892638): rel8 shape pts are
                // DIFFERENTIAL — each delta chains onto the previous point, starting from the
                // FROM node: pt_i = pt_{i-1} + delta_i, with pt_{-1} = fromNode.
                let fn_ = from_node[k];
                if !nodes.is_empty() && fn_ >= 0 && fn_ < nodes.len() as i32 {
                    let (mut ax, mut ay) = nodes[fn_ as usize];
                    mid = dvec
                        .iter()
                        .map(|&(dx, dy)| {
                            ax += dx;
                            ay += dy;
                            (ax, ay)
                        })
                        .collect();
                }
            }
        }

        // road line = [fromNode] + shapePts + [toNode]; drop near-duplicate joins. The
        // from/to endpoints are zerocells (they carry the border marker); intermediate
        // shape points are pure geometry and never merge across clusters.
        let fn_ = from_node[k];
        let tn = to_node[k];
        let mut out_pts: Vec<(i64, i64)> = Vec::new();
        let mut out_bd: Vec<bool> = Vec::new();
        if !nodes.is_empty() && fn_ >= 0 && fn_ < nodes.len() as i32 {
            out_pts.push(nodes[fn_ as usize]);
            out_bd.push(node_border.get(fn_ as usize).copied().unwrap_or(false));
        }
        for mp in mid {
            out_pts.push(mp);
            out_bd.push(false);
        }
        if !nodes.is_empty() && tn >= 0 && tn < nodes.len() as i32 {
            out_pts.push(nodes[tn as usize]);
            out_bd.push(node_border.get(tn as usize).copied().unwrap_or(false));
        }
        let thr = 2.0 / PAU;
        let mut res: Vec<(i64, i64)> = Vec::new();
        let mut res_bd: Vec<bool> = Vec::new();
        for (pnt, bd) in out_pts.iter().zip(out_bd.iter()) {
            if res.is_empty()
                || (pnt.0 - res[res.len() - 1].0).abs() as f64 > thr
                || (pnt.1 - res[res.len() - 1].1).abs() as f64 > thr
            {
                res.push(*pnt);
                res_bd.push(*bd);
            }
        }
        if res.len() < 2 {
            continue; // no usable geometry: marker / degenerate cell -> roads[k] = None
        }

        let mut names = None;
        if ann_cnt != 0 {
            names = read_names(cd, ann_off, ann_cnt);
        }
        roads[k] = Some(Road {
            pts: Some(res),
            border: res_bd,
            name: names,
            hdr,
            length: oclen,
            overlaps,
        });
    }
    Some(ClusterData {
        cid: cluster_id,
        origin: (lon, lat),
        ci2,
        ci1,
        roads,
        down: down_all,
        outline,
    })
}

// Bounded DFS following down-cell refs (resolved via each cluster's ci1 list -> neighbour
// origin -> a loaded cluster in `all`). Returns the deepest refinement level reachable from
// the given onecell (0 = it has no down-cells, i.e. already at the finest stored level).
fn down_depth(
    all: &[(String, ClusterData)],
    origin_idx: &HashMap<(i64, i64), Vec<usize>>,
    start_ci: usize,
    start_oi: i32,
    cap: usize,
) -> usize {
    let mut stack: Vec<(usize, i32, usize)> = vec![(start_ci, start_oi, 0)];
    let mut seen: std::collections::HashSet<(usize, i32)> = std::collections::HashSet::new();
    let mut maxd = 0usize;
    while let Some((ci, oi, d)) = stack.pop() {
        if !seen.insert((ci, oi)) {
            continue;
        }
        if d > maxd {
            maxd = d;
        }
        if d >= cap {
            continue;
        }
        let c = &all[ci].1;
        let Some(dl) = c.down.get(oi as usize) else { continue };
        for o in dl {
            let Some(nbr) = c.ci1.get(o.cli as usize) else { continue };
            let Some(&tci) = origin_idx.get(&(nbr.lon, nbr.lat)).and_then(|v| v.first()) else {
                continue;
            };
            if o.cell >= 0 && (o.cell as usize) < all[tci].1.down.len() {
                stack.push((tci, o.cell, d + 1));
            }
        }
    }
    maxd
}

// Collect every structurally-valid cluster start. No geographic filter here: the
// selection is geometric (does the cluster's outline footprint overlap the requested
// box) and needs the decoded outline, so it is done in parse_cluster. A coarse origin
// pre-filter here would drop large tiles whose origin sits just outside the box but whose
// extent still overlaps it — exactly the western-edge clipping we want to avoid.
fn find_clusters(d: &[u8]) -> Vec<usize> {
    let mut starts = Vec::new();
    let n = d.len();
    let stop = n.saturating_sub(0x20);
    let mut start = 0usize;
    while start < stop {
        let cluster_id = u16le(d, start);
        // NOTE: the u16@2 "hdr_flags" word is NOT a validity bit — fine-detail clusters carry
        // 0x0000 there while coarse ones carry 0x0001/0x0008. Requiring it non-zero silently
        // dropped an entire LOD tier (the residential grid). The real discriminator is the
        // lf field-list pattern + outline-field plausibility below.
        if cluster_id != 0 {
            let lf = u16le(d, start + 0x16);
            if lf & 0x30 != 0 && lf & !0x7FF == 0 {
                let ooff = u16le(d, start + 0x12) as usize;
                let ocnt = u16le(d, start + 0x14);
                let shift = d[start + 0x10];
                if shift <= 128 && ooff < BLOCK && ocnt <= 0x4000 {
                    starts.push(start);
                }
            }
        }
        start += BLOCK;
    }
    starts
}

fn extract_clusters(d: &[u8], bbox: &Option<BBox>, want_fine: bool) -> Vec<ClusterData> {
    let starts = find_clusters(d);
    let mut out = Vec::new();
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(d.len());
        if let Some(c) = parse_cluster(d, start, end, bbox, want_fine) {
            if !c.roads.is_empty() || c.outline.as_ref().map_or(false, |o| o.len() >= 3) {
                out.push(c);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// input collection: each positional arg is a .DAT file or a directory scanned
// for NAV*.DAT (one level). Duplicates are de-duplicated; order preserved.
// ---------------------------------------------------------------------------

fn collect_inputs(args: &[String]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut seen: std::collections::HashSet<std::path::PathBuf> = std::collections::HashSet::new();
    for a in args {
        let p = Path::new(a);
        if p.is_dir() {
            let rd = match fs::read_dir(p) {
                Ok(rd) => rd,
                Err(e) => {
                    eprintln!("warning: cannot read dir {}: {}", p.display(), e);
                    continue;
                }
            };
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|f| f.starts_with("NAV") && f.ends_with(".DAT"))
                .collect();
            names.sort();
            for n in names {
                push_unique(&mut files, &mut seen, p.join(n));
            }
        } else if p.is_file() {
            push_unique(&mut files, &mut seen, p.to_path_buf());
        } else {
            eprintln!("warning: not a file or dir: {}", a);
        }
    }
    files
}

fn push_unique(v: &mut Vec<PathBuf>, seen: &mut std::collections::HashSet<PathBuf>, p: PathBuf) {
    let canon = fs::canonicalize(&p).unwrap_or_else(|_| p.clone());
    if seen.insert(canon) {
        v.push(p);
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut bbox_spec = "-30,30,60,75".to_string(); // whole EUR dataset (as rnw_extract_rs)
    let mut outlines = false;
    let mut outp: Option<String> = None;
    let mut snap_m = 1.0f64; // node-snap radius in meters (0 = exact match only)
    let mut stitch = true; // merge cross-cluster boundary nodes via overlap links
    let mut snap_fallback = true; // proximity-merge unmarked boundary nodes (--no-snap off)
    let mut only_secondary = false; // --secondary: emit only the secondary LOD layer; default emits only primary
    let mut dump_cid: Option<u16> = None; // --dump N: print cluster N's down-cell structure and exit
    let mut count_box_spec: Option<String> = None; // --countbox W,S,E,N: count in-box roads across parsed clusters, then exit
    let mut level: u32 = 1; // --level N: 0 = coarse tier only, 1 (default) = include the finer tier
    let mut inputs: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" if i + 1 < args.len() => {
                outp = Some(args[i + 1].clone());
                i += 2;
            }
            "-b" if i + 1 < args.len() => {
                bbox_spec = args[i + 1].clone();
                i += 2;
            }
            "--outlines" | "-O" => {
                outlines = true;
                i += 1;
            }
            "--no-stitch" => {
                stitch = false;
                i += 1;
            }
            "--no-snap" => {
                snap_fallback = false;
                i += 1;
            }
            "--secondary" => {
                only_secondary = true;
                i += 1;
            }
            "--dump" if i + 1 < args.len() => {
                dump_cid = Some(args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("invalid --dump cid '{}'", args[i + 1]);
                    exit(1);
                }));
                i += 2;
            }
            "--countbox" if i + 1 < args.len() => {
                count_box_spec = Some(args[i + 1].clone());
                i += 2;
            }
            "--level" if i + 1 < args.len() => {
                level = args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("invalid --level '{}'", args[i + 1]);
                    exit(1);
                });
                i += 2;
            }
            "-s" if i + 1 < args.len() => {
                snap_m = parse_snap(&args[i + 1]);
                i += 2;
            }
            s if s.starts_with("-b=") => {
                bbox_spec = s[3..].to_string();
                i += 1;
            }
            s if s.starts_with("--snap=") => {
                snap_m = parse_snap(&s[7..]);
                i += 1;
            }
            s if s.starts_with("--countbox=") => {
                count_box_spec = Some(s[11..].to_string());
                i += 1;
            }
            s if s.starts_with("--level=") => {
                level = s[8..].parse().unwrap_or_else(|_| {
                    eprintln!("invalid --level '{}'", &s[8..]);
                    exit(1);
                });
                i += 1;
            }
            "--help" | "-h" => {
                usage();
                return;
            }
            other => {
                inputs.push(other.to_string());
                i += 1;
            }
        }
    }

    if inputs.is_empty() {
        usage();
        exit(1);
    }
    let bbox = match BBox::parse(&bbox_spec) {
        Some(b) => Some(b),
        None if bbox_spec.eq_ignore_ascii_case("none") => None,
        None => {
            eprintln!("invalid -b '{}' (expected W,S,E,N in degrees or 'none')", bbox_spec);
            exit(1);
        }
    };

    // This dataset has exactly two cluster tiers: the coarse layer and the finer layer
    // (u16@2==0 clusters, the dense residential grid). Down-cell refinement from a coarse
    // onecell reaches only one step deeper (max_depth=1), so there is no level beyond 1.
    if level > 1 {
        eprintln!("invalid --level {} (this dataset has 2 tiers: 0 = coarse, 1 = finest)", level);
        exit(1);
    }
    let want_fine = level >= 1;

    let files = collect_inputs(&inputs);
    if files.is_empty() {
        eprintln!("error: no NAV*.DAT inputs found");
        exit(1);
    }

    let snap_pau = (snap_m * M_TO_PAU).round() as i64;

    // Phase A: parse every cluster in every file up front. Cross-cluster overlap
    // links can point into other files, so all geometry must be available before
    // stitching. `all` is (source label, parsed cluster), in input order.
    let mut all: Vec<(String, ClusterData)> = Vec::new();
    for fp in &files {
        let label = fp.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let d = match fs::read(fp) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("warning: cannot read {}: {}", fp.display(), e);
                continue;
            }
        };
        for c in extract_clusters(&d, &bbox, want_fine) {
            all.push((label.clone(), c));
        }
    }

    // Global index: cluster origin (lon,lat) -> indices into `all`. Origin is a
    // near-unique key (only a handful of degenerate/same-file collisions); ci2
    // neighbor records carry the same origin, so overlaps resolve through it.
    let mut origin_idx: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (i, (_, c)) in all.iter().enumerate() {
        origin_idx.entry(c.origin).or_default().push(i);
    }

    if let Some(spec) = count_box_spec {
        let qbb = match BBox::parse(&spec) {
            Some(b) => b,
            None => {
                eprintln!("invalid --countbox '{}' (expected W,S,E,N in degrees)", spec);
                exit(1);
            }
        };
        let mut by_class: HashMap<String, u64> = HashMap::new();
        let mut total = 0u64;
        let mut clusters_hit = 0usize;
        let mut clusters_hit_selected = 0usize;
        for (_, c) in all.iter() {
            // Would this cluster be selected by geometric (outline) selection?
            let sel = match &c.outline {
                Some(ol) if !ol.is_empty() => {
                    let (w0, s0) = ol[0];
                    let (mut w, mut e) = (w0, w0);
                    let (mut s, mut n) = (s0, s0);
                    for &(x, y) in ol.iter() {
                        if x < w { w = x; }
                        if x > e { e = x; }
                        if y < s { s = y; }
                        if y > n { n = y; }
                    }
                    qbb.intersects_pau(w as f64, s as f64, e as f64, n as f64)
                }
                _ => qbb.contains(c.origin.0 as f64, c.origin.1 as f64),
            };
            let mut cluster_has = false;
            for r in c.roads.iter().flatten() {
                let Some(pts) = &r.pts else { continue };
                if !pts.iter().any(|&(x, y)| qbb.contains(x as f64, y as f64)) {
                    continue;
                }
                cluster_has = true;
                total += 1;
                let rc = r.hdr & 0x7;
                let nc = (r.hdr >> 4) & 0x7;
                let link = (r.hdr >> 13) & 1;
                let hw = match display_class(rc as i64, nc as i64) {
                    Some(dc) => highway_tag(dc, link == 1).to_string(),
                    None => format!("rc{}", rc),
                };
                *by_class.entry(hw).or_insert(0) += 1;
            }
            if cluster_has {
                clusters_hit += 1;
                if sel {
                    clusters_hit_selected += 1;
                }
            }
        }
        println!(
            "--countbox {} : {} road(s) in box, from {} cluster(s); {} of those would be outline-selected",
            spec, total, clusters_hit, clusters_hit_selected
        );
        let mut vc: Vec<(&String, &u64)> = by_class.iter().collect();
        vc.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        for (k, v) in vc {
            println!("    {:<20} {}", k, v);
        }
        exit(0);
    }

    if let Some(want) = dump_cid {
        let Some((di, (label, c))) = all
            .iter()
            .enumerate()
            .find(|(_, (l, cc))| cc.cid == want && !l.is_empty())
        else {
            eprintln!("--dump: cluster {} not found among the selected clusters", want);
            exit(1);
        };
        let (lon0, lat0) = c.origin;
        println!(
            "cluster {} @ {}: origin ({:.6},{:.6})  ci2={} roads={} onecells={}",
            c.cid,
            label,
            lon0 as f64 / PAU,
            lat0 as f64 / PAU,
            c.ci2.len(),
            c.roads.iter().filter(|r| r.is_some()).count(),
            c.roads.len()
        );
        println!("  ci1 table (index -> neighbour) [down-cells index THIS]:");
        for (ci, n) in c.ci1.iter().enumerate() {
            let loaded = origin_idx.get(&(n.lon, n.lat)).map(|v| v.len()).unwrap_or(0);
            println!(
                "    [{:>3}] num={}  ({:.6},{:.6})  loaded_here={}",
                ci,
                n.num,
                n.lon as f64 / PAU,
                n.lat as f64 / PAU,
                loaded
            );
        }
        let mut with_down = 0usize;
        let mut total_refs = 0usize;
        for (k, dl) in c.down.iter().enumerate() {
            if dl.is_empty() {
                continue;
            }
            with_down += 1;
            total_refs += dl.len();
            let geom = c.roads[k].is_some();
            let sec = c.roads[k].as_ref().map(|r| (r.hdr >> 15) & 1).unwrap_or(0);
            let refs: Vec<String> = dl
                .iter()
                .map(|o| {
                    let n = c.ci1.get(o.cli as usize);
                    match n {
                        Some(n) => format!(
                            "ci1[{}]->num{}/({:.5},{:.5}) oc{}",
                            o.cli,
                            n.num,
                            n.lon as f64 / PAU,
                            n.lat as f64 / PAU,
                            o.cell
                        ),
                        None => format!("ci1[{}]=? oc{}", o.cli, o.cell),
                    }
                })
                .collect();
            if with_down <= 40 {
                println!(
                    "  oc[{}] geom={} sec={} down={} : {}",
                    k, geom, sec, dl.len(), refs.join(" | ")
                );
            }
        }
        // Recursion depth: follow down-cells (via ci1 -> origin) from each onecell and report
        // how many refinement levels exist below this cluster.
        let mut depths: Vec<usize> = Vec::new();
        for (k, dl) in c.down.iter().enumerate() {
            if dl.is_empty() {
                continue;
            }
            depths.push(down_depth(&all, &origin_idx, di, k as i32, 8));
        }
        let mut hist: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
        for d in &depths {
            *hist.entry(*d).or_insert(0) += 1;
        }
        let dist: Vec<String> = hist.iter().map(|(d, n)| format!("L{}={}", d, n)).collect();
        println!(
            "  summary: onecells_with_down={} total_down_refs={} depth_hist[{}] max_depth={}",
            with_down,
            total_refs,
            dist.join(" "),
            depths.iter().max().copied().unwrap_or(0)
        );
        let _ = di;
        return;
    }

    // Phase B: emit every road + outline, recording each onecell's endpoint node
    // ids (indexed [cluster][raw onecell]) so overlap links can be resolved.
    let mut ob = OsmBuilder::new(snap_pau, snap_fallback);
    let (mut n_roads, mut n_named, mut n_geom, mut n_outlines) = (0u64, 0u64, 0u64, 0u64);
    // Which LOD layer to emit: 0 = primary (default, what the app renders), 1 = secondary.
    let want_sec: u32 = if only_secondary { 1 } else { 0 };
    let mut n_omitted = 0u64; // onecells of the other layer, not emitted this run
    let mut n_refined = 0u64; // coarse-tier roads dropped because their fine breakdown is present
    let mut ends: Vec<Vec<Option<(i64, i64)>>> = Vec::with_capacity(all.len());
    for (label, c) in all.iter() {
        let mut cends: Vec<Option<(i64, i64)>> = vec![None; c.roads.len()];
        for (oi, r) in c.roads.iter().enumerate() {
            let Some(r) = r else { continue };
            // Each road can be stored as a primary (full detail) and/or a secondary (coarser)
            // onecell. The runtime always renders the primary (oGetPrimaryOverlap @0x0088c884),
            // so by default we emit primaries only; --secondary emits just the coarser layer.
            if ((r.hdr >> 15) & 1) != want_sec {
                n_omitted += 1;
                continue;
            }
            // At --level 1 the fine tier supersedes the coarse copy of a road. A coarse onecell is
            // "broken down" into finer sub-segments via its down-cells (u16BreakDownNextOC); when
            // every one of those finer onecells is present in this run, drop the coarse duplicate so
            // the same road is drawn once, at full detail. If any down-cell target is missing we keep
            // the coarse copy — de-duplication must never lose a road.
            if want_fine {
                let dl = &c.down[oi];
                if !dl.is_empty()
                    && dl.iter().all(|o| {
                        c.ci1.get(o.cli as usize).and_then(|n| {
                            origin_idx
                                .get(&(n.lon, n.lat))
                                .and_then(|v| v.first())
                                .map(|&ti| all[ti].1.roads.get(o.cell as usize).is_some_and(|rr| rr.is_some()))
                        }) == Some(true)
                    })
                {
                    n_refined += 1;
                    continue;
                }
            }
            n_roads += 1;
            if r.name.is_some() {
                n_named += 1;
            }
            let pts = match &r.pts {
                Some(p) if !p.is_empty() => p,
                _ => continue,
            };
            n_geom += 1;
            let h = r.hdr;
            let rc = h & 0x7;
            let nc = (h >> 4) & 0x7;
            let rt = (h >> 8) & 0xF;
            let gateway = (h >> 3) & 1;
            let tunnel = (h >> 7) & 1;
            let link = (h >> 13) & 1;
            let loc_link = (h >> 14) & 1;
            let sec = (h >> 15) & 1;
            let cpx_crossing = (h >> 17) & 1;
            let cpx_road = (h >> 18) & 1;
            let bdata = (h >> 19) & 1;
            let oneway_from = (h >> 20) & 1;
            let oneway_to = (h >> 21) & 1;
            let ferry = (h >> 23) & 1;
            let restricted = (h >> 26) & 1;
            let blocked = (h >> 27) & 1;
            let part_of_object = (h >> 29) & 1;
            let freeway = (h >> 30) & 1;
            let bridge = (h >> 31) & 1;
            let mut tags = Vec::new();
            if let Some(dc) = display_class(rc as i64, nc as i64) {
                let hw = highway_tag(dc, link == 1);
                if !hw.is_empty() {
                    tags.push(tag("highway", hw));
                }
            }
            if tunnel == 1 {
                tags.push(tag("tunnel", "yes"));
            }
            if bridge == 1 {
                tags.push(tag("bridge", "yes"));
            }
            if rt == 2 {
                tags.push(tag("junction", "roundabout"));
            }
            if oneway_from == 1 && oneway_to == 0 {
                tags.push(tag("oneway", "yes"));
            } else if oneway_to == 1 && oneway_from == 0 {
                tags.push(tag("oneway", "-1"));
            }
            if let Some(names) = &r.name {
                if let Some(n0) = names.first() {
                    tags.push(tag("name", n0.clone()));
                }
                if names.len() > 1 {
                    tags.push(tag("name:alt", names[1..].join("; ")));
                }
            }
            tags.push(tag("rn_class", rc.to_string()));
            tags.push(tag("rn_netclass", nc.to_string()));
            tags.push(tag("rn_roadtype", rt.to_string()));
            tags.push(tag("rn_link", link.to_string()));
            tags.push(tag("rn_sec", sec.to_string()));
            tags.push(tag("rn_freeway", freeway.to_string()));
            if r.length != 0 {
                tags.push(tag("rn_length", r.length.to_string()));
            }
            if gateway == 1 {
                tags.push(tag("rn_gateway", "yes"));
            }
            if tunnel == 1 {
                tags.push(tag("rn_tunnel", "yes"));
            }
            if loc_link == 1 {
                tags.push(tag("rn_loc_link", "yes"));
            }
            if cpx_crossing == 1 {
                tags.push(tag("rn_cpx_crossing", "yes"));
            }
            if cpx_road == 1 {
                tags.push(tag("rn_cpx_road", "yes"));
            }
            if bdata == 1 {
                tags.push(tag("rn_bdata", "yes"));
            }
            if oneway_from == 1 {
                tags.push(tag("rn_oneway_from", "yes"));
            }
            if oneway_to == 1 {
                tags.push(tag("rn_oneway_to", "yes"));
            }
            if ferry == 1 {
                tags.push(tag("rn_ferry", "yes"));
            }
            if restricted == 1 {
                tags.push(tag("rn_restricted", "yes"));
            }
            if blocked == 1 {
                tags.push(tag("rn_blocked", "yes"));
            }
            if part_of_object == 1 {
                tags.push(tag("rn_part_of_object", "yes"));
            }
            if bridge == 1 {
                tags.push(tag("rn_bridge", "yes"));
            }
            tags.push(tag("rnw_file", label.clone()));
            tags.push(tag("rnw_cluster", c.cid.to_string()));
            tags.push(tag("rnw_oncell_index", oi.to_string()));
            if let Some(ft) = ob.add_line(pts, &r.border, tags) {
                cends[oi] = Some(ft);
            }
        }
        if outlines {
            if let Some(ol) = &c.outline {
                let mut otags = vec![tag("rnw:outline", "yes")];
                otags.push(tag("rnw_file", label.clone()));
                otags.push(tag("rnw_cluster", c.cid.to_string()));
                if ob.add_polygon(ol, otags) {
                    n_outlines += 1;
                }
            }
        }
        ends.push(cends);
    }

    // Phase C: resolve overlap links -> union the shared boundary node of each
    // linked pair. For a link from onecell k in cluster C to ci2[cli].cell, pick
    // the closest endpoint pair (this onecell x neighbor onecell) and merge their
    // OSM nodes — what the runtime's bRelevantCrossingBetween does by following
    // the overlap link before any location-based fallback.
    let mut n_ovl_total = 0u64;
    let mut n_ovl_merged = 0u64;
    if stitch {
        let mut uf = UnionFind::new();
        // Resolve an onecell reference to the endpoint of the onecell we actually emitted this
        // run. A boundary overlap link often names its twin in the *other* LOD layer (the one we
        // dropped), so follow that twin's own overlaps to the first emitted copy — the same
        // substitution the runtime makes in oGetPrimaryOverlap @0x0088c884. Without this, every
        // cross-layer link is unresolvable and primary-only stitching collapses to 0 merges.
        let resolve = |ci: usize, oi: usize| -> Option<(usize, usize)> {
            if ends[ci].get(oi)?.is_some() {
                return Some((ci, oi));
            }
            let r = all[ci].1.roads.get(oi)?.as_ref()?;
            for ov in &r.overlaps {
                let nbr = all[ci].1.ci2.get(ov.cli as usize)?;
                let ncis = origin_idx.get(&(nbr.lon, nbr.lat))?;
                let nj = ov.cell as usize;
                let mut ordered: Vec<usize> = ncis.iter().copied().filter(|&x| x != ci).collect();
                ordered.sort_by_key(|&x| all[x].1.cid as i64 != nbr.num);
                for &nci in &ordered {
                    if nj >= all[nci].1.roads.len() {
                        continue;
                    }
                    // first twin actually emitted this run (primary by default, secondary under
                    // --secondary) — mirrors oGetPrimaryOverlap substituting for the other layer.
                    if ends[nci].get(nj)?.is_some() {
                        return Some((nci, nj));
                    }
                }
            }
            None
        };
        for (ci, (_, c)) in all.iter().enumerate() {
            for (oi, r) in c.roads.iter().enumerate() {
                let Some(r) = r else { continue };
                let Some((fi, ti)) = ends[ci][oi] else { continue };
                let Some(pts) = &r.pts else { continue };
                if pts.len() < 2 {
                    continue;
                }
                let e0 = pts[0];
                let e1 = *pts.last().unwrap();
                for ov in &r.overlaps {
                    n_ovl_total += 1;
                    let Some(nbr) = c.ci2.get(ov.cli as usize) else { continue };
                    let Some(ncis) = origin_idx.get(&(nbr.lon, nbr.lat)) else { continue };
                    let nj = ov.cell as usize;
                    // Order candidates so the cluster whose id matches the ci2-recorded
                    // number comes first — this disambiguates the rare same-origin
                    // collisions without needing the full 8-byte ClusterID.
                    let mut ordered: Vec<usize> = ncis.iter().copied().filter(|&x| x != ci).collect();
                    ordered.sort_by_key(|&x| all[x].1.cid as i64 != nbr.num);
                    for &nci in &ordered {
                        if nj >= all[nci].1.roads.len() {
                            continue;
                        }
                        let Some((rci, roi)) = resolve(nci, nj) else {
                            continue;
                        };
                        let Some(nr) = all[rci].1.roads.get(roi).and_then(|x| x.as_ref()) else {
                            continue;
                        };
                        let Some((nf, nt)) = ends[rci].get(roi).copied().flatten() else {
                            continue;
                        };
                        let Some(npts) = &nr.pts else { continue };
                        if npts.len() < 2 {
                            continue;
                        }
                        let n0 = npts[0];
                        let n1 = *npts.last().unwrap();
                        // closest endpoint pair among the 4 combinations
                        let mut best_d2 = i128::MAX;
                        let mut best: Option<(i64, i64)> = None; // (this_id, nbr_id)
                        for &(ec, eid) in &[(e0, fi), (e1, ti)] {
                            for &(nc_, nid) in &[(n0, nf), (n1, nt)] {
                                let dx = ec.0 - nc_.0;
                                let dy = ec.1 - nc_.1;
                                let d2 = (dx as i128) * (dx as i128) + (dy as i128) * (dy as i128);
                                if d2 < best_d2 {
                                    best_d2 = d2;
                                    best = Some((eid, nid));
                                }
                            }
                        }
                        if let Some((a, b)) = best {
                            uf.union(a, b);
                            n_ovl_merged += 1;
                        }
                        break; // resolved via first valid neighbor cluster at this origin
                    }
                }
            }
        }
        ob.apply_node_merges(&uf);
    }

    let (nodes, ways) = ob.finish();
    let osm = OsmData {
        version: "0.6".to_string(),
        generator: GENERATOR.to_string(),
        nodes,
        ways,
    };

    let mut xml_buf = String::new();
    let mut serializer = quick_xml::se::Serializer::new(&mut xml_buf);
    serializer.indent(' ', 2);
    if let Err(e) = osm.serialize(serializer) {
        eprintln!("error serializing OSM: {}", e);
        exit(1);
    }

    match &outp {
        Some(path) => match fs::File::create(path) {
            Ok(f) => {
                let mut w = BufWriter::new(f);
                if let Err(e) = write_osm(&mut w, &xml_buf) {
                    eprintln!("error writing {}: {}", path, e);
                    exit(1);
                }
            }
            Err(e) => {
                eprintln!("error creating {}: {}", path, e);
                exit(1);
            }
        },
        None => {
            let stdout = io::stdout();
            let mut w = BufWriter::new(stdout.lock());
            if let Err(e) = write_osm(&mut w, &xml_buf) {
                eprintln!("error writing stdout: {}", e);
                exit(1);
            }
        }
    }

    let refined_note = if n_refined > 0 {
        format!(" refined_dropped={n_refined}")
    } else {
        String::new()
    };
    let sec_note = format!(
        "{} omitted={}{}",
        if only_secondary { "layer=secondary" } else { "layer=primary" },
        n_omitted,
        refined_note
    );
    if stitch {
        eprintln!(
            "files={} clusters={} roads={} named={} with_geom={} outlines={} overlaps={}/{} {}",
            files.len(),
            all.len(),
            n_roads,
            n_named,
            n_geom,
            n_outlines,
            n_ovl_merged,
            n_ovl_total,
            sec_note
        );
    } else {
        eprintln!(
            "files={} clusters={} roads={} named={} with_geom={} outlines={} (stitch off) {}",
            files.len(),
            all.len(),
            n_roads,
            n_named,
            n_geom,
            n_outlines,
            sec_note
        );
    }
}

fn write_osm<W: Write>(w: &mut W, body: &str) -> io::Result<()> {
    w.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")?;
    w.write_all(body.as_bytes())?;
    w.write_all(b"\n")
}

fn usage() {
    eprintln!(
        "rnw2osm_rs - convert Bosch RNW NAV*.DAT cluster files to OSM XML for visualization\n\
         \n\
            usage: rnw2osm_rs <NAV*.DAT | dir>... [-o OUT.osm] [--outlines] [-b W,S,E,N|none]\n\
                    [-s M] [--no-stitch] [--no-snap] [--secondary] [--level N]\n\
           \n\
             one or more INPUTs, each a NAV*.DAT file or a directory to scan for them\n\
             -o FILE       write OSM XML to FILE (default: stdout)\n\
             --outlines    also emit each cluster's boundary outline as a closed polygon way\n\
             -b W,S,E,N    geographic sanity box for the 16KB cluster scan (degrees);\n\
                           'none' disables it. Default: -30,30,60,75 (whole EUR dataset).\n\
             Boundary-node merging (each cluster stores its own copy of a junction, a few\n\
             PAU apart), applied in the runtime's order — overlap links first, then marker,\n\
             then proximity:\n\
               -s M          snap radius in meters for the marker + proximity merges\n\
                             (default 1.0; 0 = exact match only, i.e. links-only). A node with\n\
                             the RNW border marker (zerocell rim / cpx-crossing flag, or a 0x31\n\
                             annotation — the runtime's bBordersObjectAtTo/From test) merges with\n\
                             its nearest marked twin; an unmarked junction merges with any nearby\n\
                             twin. The latter is what keeps the network connected.\n\
              --no-stitch   disable overlap-link stitching (the runtime's primary join).\n\
               --no-snap     disable the proximity fallback for UNMARKED junctions; only marked\n\
                             nodes (and overlap links) are merged. Purest faithful mode, but the\n\
                             network fragments because most boundary junctions carry no marker.\n\
              Detail level (LOD) — two independent axes:\n\
                (1) Cluster tier (--level). The NAV files hold two interleaved cluster tiers: a coarse\n\
                layer and a finer layer (clusters whose u16@2 word is 0x0000) that carries the dense\n\
                residential grid. A coarse onecell refines into the fine tier via down-cells, one step\n\
                deep (max_depth=1). Both tiers are needed for a complete street map.\n\
                 --level N     0 = coarse tier only (the lightweight overview); 1 (default) = include the\n\
                               finer tier as well. This dataset has no level beyond 1. At level 1 a coarse\n\
                               road that is fully refined into fine sub-segments present in this run is drawn\n\
                               once (the fine version), not twice — de-duplication never drops a road that has\n\
                               no fine counterpart, and it never loses a name (summary shows refined_dropped=N).\n\
                (2) Primary/secondary twin (--secondary). Within a tier, each road can be stored as two\n\
                coincident onecells — a primary (full shape, usually named) and a secondary (coarser,\n\
                often unnamed), flagged bIsSecundary (header bit 15, tag rn_sec). The runtime always\n\
                renders the PRIMARY twin (oGetPrimaryOverlap). By default we emit the primary layer only —\n\
                each road once, at full detail. Every way carries rn_sec (0=primary, 1=secondary).\n\
                 --secondary   emit ONLY the secondary LOD layer instead of the primary one. For inspecting\n\
                               the coarser representation in isolation; not a usable map on its own.\n\
              Diagnostics:\n\
                 --dump CID     print one cluster's neighbour/down-cell structure and exit\n\
                 --countbox W,S,E,N   count roads falling in a box across parsed clusters (by class) and exit\n"
    );
}
