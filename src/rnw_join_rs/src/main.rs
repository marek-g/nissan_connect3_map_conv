// Join RNW-extracted road names/attributes onto MAP OSM XML roads.
// Rust port of rnw_join.py; matching logic is a straight port (same f64
// operations in the same order). Output is canonical OSM XML in the map2osm
// layout: one line per <node>, <way> with all <nd> on one line and all
// <tag> on one line before </way>.
//
// Usage: rnw_join_rs <rnw_roads.jsonl> <map.osm> <out.osm>
//
// A MAP road way is [fromNode] + shapePts + [toNode] of ONE or several chained
// RNW onecells (MAP merges consecutive same-class segments). We collect all
// RNW roads that lie ALONG the MAP polyline ("components": >=80% of their
// points within 30m of the MAP line) and combine their names/attributes.

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io::{BufWriter, Write};
use std::process::exit;

const CELL: f64 = 0.05;
const GENERATOR: &str = "rnw_join (Bosch TravelMap converter)";

// ---------------------------------------------------------------- JSON input

enum JVal {
    Null,
    Int(i64),
    F64(f64),
    Str(String),
    Arr(Vec<JVal>),
    Obj(Vec<(String, JVal)>),
}

impl JVal {
    fn get(&self, k: &str) -> Option<&JVal> {
        if let JVal::Obj(m) = self {
            m.iter().rev().find(|(kk, _)| kk == k).map(|(_, v)| v)
        } else {
            None
        }
    }
}

struct JParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> JParser<'a> {
    fn ws(&mut self) {
        while let Some(&c) = self.b.get(self.i) {
            if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn value(&mut self) -> JVal {
        self.ws();
        match self.b.get(self.i) {
            Some(b'n') => {
                self.i += 4; // null
                JVal::Null
            }
            Some(b't') => {
                self.i += 4; // true (not expected in our data)
                JVal::Int(1)
            }
            Some(b'f') => {
                self.i += 5; // false
                JVal::Int(0)
            }
            Some(b'"') => JVal::Str(self.string()),
            Some(b'[') => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b']') => {
                            self.i += 1;
                            break;
                        }
                        _ => v.push(self.value()),
                    }
                    self.ws();
                    if self.b.get(self.i) == Some(&b',') {
                        self.i += 1;
                    }
                }
                JVal::Arr(v)
            }
            Some(b'{') => {
                self.i += 1;
                let mut m = Vec::new();
                loop {
                    self.ws();
                    match self.b.get(self.i) {
                        Some(b'}') => {
                            self.i += 1;
                            break;
                        }
                        _ => {
                            let k = self.string();
                            self.ws();
                            self.i += 1; // ':'
                            m.push((k, self.value()));
                        }
                    }
                    self.ws();
                    if self.b.get(self.i) == Some(&b',') {
                        self.i += 1;
                    }
                }
                JVal::Obj(m)
            }
            _ => {
                let start = self.i;
                while let Some(&c) = self.b.get(self.i) {
                    if c == b',' || c == b']' || c == b'}' || c == b' ' || c == b'\n' {
                        break;
                    }
                    self.i += 1;
                }
                let s = std::str::from_utf8(&self.b[start..self.i]).unwrap_or("0");
                if s.contains('.') || s.contains('e') || s.contains('E') {
                    JVal::F64(s.parse::<f64>().unwrap_or(0.0))
                } else {
                    match s.parse::<i64>() {
                        Ok(v) => JVal::Int(v),
                        Err(_) => JVal::F64(s.parse::<f64>().unwrap_or(0.0)),
                    }
                }
            }
        }
    }

    fn string(&mut self) -> String {
        self.i += 1; // opening quote
        let mut out = Vec::new();
        loop {
            let c = self.b[self.i];
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self.b[self.i];
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0C),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let h = std::str::from_utf8(&self.b[self.i..self.i + 4])
                                .unwrap_or("0000");
                            self.i += 4;
                            let cp = u32::from_str_radix(h, 16).unwrap_or(0);
                            let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => out.push(e),
                    }
                }
                c => out.push(c),
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

// --------------------------------------------------------------- OSM parsing

struct OsmEl {
    tag: String,
    attrs: Vec<(String, String)>,
    nds: Vec<String>,
    tags: Vec<(String, String)>,
}

fn attr<'a>(el: &'a OsmEl, k: &str) -> Option<&'a str> {
    el.attrs.iter().rev().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str())
}
fn attr_i64(el: &OsmEl, k: &str) -> i64 {
    attr(el, k).unwrap().parse().unwrap()
}
fn attr_f64(el: &OsmEl, k: &str) -> f64 {
    attr(el, k).unwrap().parse().unwrap()
}

fn unescape(raw: &[u8]) -> String {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'&' {
            if let Some(rel) = raw[i..].iter().position(|&c| c == b';') {
                let ent = &raw[i + 1..i + rel];
                let decoded = std::str::from_utf8(ent).ok().and_then(|name| {
                    match name {
                        "amp" => Some("&".to_string()),
                        "lt" => Some("<".to_string()),
                        "gt" => Some(">".to_string()),
                        "quot" => Some("\"".to_string()),
                        "apos" => Some("'".to_string()),
                        _ => name
                            .strip_prefix('#')
                            .and_then(|digits| {
                                let v = if let Some(h) = digits
                                    .strip_prefix('x')
                                    .or_else(|| digits.strip_prefix('X'))
                                {
                                    u32::from_str_radix(h, 16).ok()
                                } else {
                                    digits.parse::<u32>().ok()
                                };
                                v.and_then(char::from_u32).map(|c| c.to_string())
                            }),
                    }
                });
                if let Some(s) = decoded {
                    out.extend_from_slice(s.as_bytes());
                    i += rel + 1;
                    continue;
                }
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

struct OsmStream<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> OsmStream<'a> {
    fn new(b: &'a [u8]) -> Result<Self, String> {
        let mut s = OsmStream { b, i: 0 };
        if !b.starts_with(b"<?xml") {
            return Err("not an OSM XML file".into());
        }
        while !b[s.i..].starts_with(b"?>") {
            s.i += 1;
        }
        s.i += 2;
        s.skip_text();
        let (tag, _, _) = s.start_tag();
        if tag != "osm" {
            return Err(format!("not an OSM XML file: root is <{}>", tag));
        }
        Ok(s)
    }

    fn skip_text(&mut self) {
        while self.i < self.b.len() && self.b[self.i] != b'<' {
            self.i += 1;
        }
    }

    fn name(&mut self) -> String {
        let start = self.i;
        while let Some(&c) = self.b.get(self.i) {
            if c.is_ascii_alphanumeric() || c == b'-' || c == b'_' || c == b':' {
                self.i += 1;
            } else {
                break;
            }
        }
        std::str::from_utf8(&self.b[start..self.i]).unwrap_or("").to_string()
    }

    // at '<'; returns (name, attrs, self_closing)
    fn start_tag(&mut self) -> (String, Vec<(String, String)>, bool) {
        self.i += 1;
        let name = self.name();
        let mut attrs = Vec::new();
        loop {
            while let Some(&c) = self.b.get(self.i) {
                if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                    self.i += 1;
                } else {
                    break;
                }
            }
            match self.b.get(self.i) {
                Some(b'/') => {
                    self.i += 2; // "/>"
                    return (name, attrs, true);
                }
                Some(b'>') => {
                    self.i += 1;
                    return (name, attrs, false);
                }
                _ => {
                    let k = self.name();
                    while let Some(&c) = self.b.get(self.i) {
                        if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                            self.i += 1;
                        } else {
                            break;
                        }
                    }
                    self.i += 1; // '='
                    let quote = self.b[self.i];
                    self.i += 1;
                    let vs = self.i;
                    while self.b[self.i] != quote {
                        self.i += 1;
                    }
                    let v = unescape(&self.b[vs..self.i]);
                    self.i += 1;
                    attrs.push((k, v));
                }
            }
        }
    }

    fn skip_element_body(&mut self, tag: &str) {
        let mut depth = 1;
        while depth > 0 {
            self.skip_text();
            if self.b[self.i] == b'<' && self.b.get(self.i + 1) == Some(&b'/') {
                self.i += 2;
                let n = self.name();
                while let Some(&c) = self.b.get(self.i) {
                    if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                self.i += 1; // '>'
                if n == tag {
                    depth -= 1;
                }
            } else {
                let (n, _, sc) = self.start_tag();
                if !sc && n == tag {
                    depth += 1;
                }
            }
        }
    }

    // next top-level element, or None at </osm>
    fn next_element(&mut self) -> Option<OsmEl> {
        self.skip_text();
        if self.i >= self.b.len() {
            return None;
        }
        if self.b[self.i] == b'<' && self.b.get(self.i + 1) == Some(&b'/') {
            return None; // </osm>
        }
        let (tag, attrs, sc) = self.start_tag();
        let mut el = OsmEl {
            tag,
            attrs,
            nds: Vec::new(),
            tags: Vec::new(),
        };
        if sc {
            return Some(el);
        }
        loop {
            self.skip_text();
            if self.b[self.i] == b'<' && self.b.get(self.i + 1) == Some(&b'/') {
                self.skip_element_body(&el.tag); // consumes the end tag
                return Some(el);
            }
            let (ctag, cattrs, csc) = self.start_tag();
            if !csc {
                self.skip_element_body(&ctag);
                continue;
            }
            match ctag.as_str() {
                "nd" => el.nds.push(cattrs[0].1.clone()),
                "tag" => el.tags.push((cattrs[0].1.clone(), cattrs[1].1.clone())),
                _ => {}
            }
        }
    }
}

// ------------------------------------------------------------------- joining

struct RnwRoad {
    pts: Vec<(f64, f64)>,
    minx: f64,
    maxx: f64,
    miny: f64,
    maxy: f64,
    name: Option<String>,
    rc: Option<i64>,
    nc: Option<i64>,
    rt: Option<i64>,
    link: Option<i64>,
    sec: Option<i64>,
    fw: Option<i64>,
}

// rnw_tclMAPConverter::enConvertRoadSubattrDisplayClass @0x00888b14 — the
// runtime's own road rendering hierarchy, derived from (roadClass, networkClass).
// Lower value = more important road. dc=2 is the motorway tier (the only class
// that carries the freeway bit); dc=12/13 are the common local roads.
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

// Map the runtime display class onto the OSM highway=* hierarchy (monotonic in
// importance). rn_link (onecell bit 13, bIsLink) marks a ramp/connecting road,
// which OSM encodes with the *_link variant for the major classes.
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

fn load_rnw(path: &str) -> (Vec<RnwRoad>, HashMap<(i32, i32), Vec<u32>>) {
    let data = match fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read {}: {}", path, e);
            exit(1);
        }
    };
    let mut roads: Vec<RnwRoad> = Vec::new();
    let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
    for line in data.split(|&c| c == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut p = JParser { b: line, i: 0 };
        let v = p.value();
        let pts = match v.get("pts") {
            Some(JVal::Arr(a)) if a.len() >= 2 => {
                Some(a.iter().filter_map(|pt| {
                    if let JVal::Arr(xy) = pt {
                        if xy.len() == 2 {
                            let x = match &xy[0] {
                                JVal::F64(f) => *f,
                                JVal::Int(i) => *i as f64,
                                _ => return None,
                            };
                            let y = match &xy[1] {
                                JVal::F64(f) => *f,
                                JVal::Int(i) => *i as f64,
                                _ => return None,
                            };
                            Some((x, y))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }).collect::<Vec<(f64, f64)>>())
            }
            _ => None,
        };
        let pts = match pts {
            Some(p) if p.len() >= 2 => p,
            _ => continue,
        };
        let mut minx = f64::INFINITY;
        let mut maxx = f64::NEG_INFINITY;
        let mut miny = f64::INFINITY;
        let mut maxy = f64::NEG_INFINITY;
        for &(x, y) in &pts {
            if x < minx {
                minx = x;
            }
            if x > maxx {
                maxx = x;
            }
            if y < miny {
                miny = y;
            }
            if y > maxy {
                maxy = y;
            }
        }
        let i = roads.len() as u32;
        roads.push(RnwRoad {
            name: v.get("name").and_then(|x| match x {
                JVal::Str(s) => Some(s.clone()),
                _ => None,
            }),
            rc: v.get("rc").and_then(|x| match x {
                JVal::Int(i) => Some(*i),
                _ => None,
            }),
            nc: v.get("nc").and_then(|x| match x {
                JVal::Int(i) => Some(*i),
                _ => None,
            }),
            rt: v.get("rt").and_then(|x| match x {
                JVal::Int(i) => Some(*i),
                _ => None,
            }),
            link: v.get("link").and_then(|x| match x {
                JVal::Int(i) => Some(*i),
                _ => None,
            }),
            sec: v.get("sec").and_then(|x| match x {
                JVal::Int(i) => Some(*i),
                _ => None,
            }),
            fw: v.get("fw").and_then(|x| match x {
                JVal::Int(i) => Some(*i),
                _ => None,
            }),
            pts,
            minx,
            maxx,
            miny,
            maxy,
        });
        for &(x, y) in &roads[i as usize].pts {
            grid.entry(((x / CELL) as i32, (y / CELL) as i32))
                .or_default()
                .push(i);
        }
    }
    (roads, grid)
}

// Point-segment distance in meters (local equirectangular).
fn seg_dist_m(px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let k = ((ay + by) / 2.0).to_radians().cos();
    let axk = ax * k;
    let bxa = bx * k;
    let pxa = px * k;
    let dx = bxa - axk;
    let dy = by - ay;
    let d = if dx == 0.0 && dy == 0.0 {
        (pxa - axk).hypot(py - ay)
    } else {
        let t = ((pxa - axk) * dx + (py - ay) * dy) / (dx * dx + dy * dy);
        let t = t.max(0.0).min(1.0);
        (pxa - (axk + t * dx)).hypot(py - (ay + t * dy))
    };
    d * 111132.0
}

fn seg_len_m(ax: f64, ay: f64, bx: f64, by: f64) -> f64 {
    let k = ((ay + by) / 2.0).to_radians().cos();
    ((bx - ax) * k).hypot(by - ay) * 111132.0
}

fn pt_to_poly_m(px: f64, py: f64, poly: &[(f64, f64)]) -> f64 {
    let mut d = 1e9;
    for i in 0..poly.len() - 1 {
        let dd = seg_dist_m(px, py, poly[i].0, poly[i].1, poly[i + 1].0, poly[i + 1].1);
        if dd < d {
            d = dd;
            if d < 2.0 {
                break;
            }
        }
    }
    d
}

// Mean distance of RNW points to the MAP polyline (meters), or None if the
// RNW road is not a component of the MAP road.
fn component_score(rnw_pts: &[(f64, f64)], map_poly: &[(f64, f64)]) -> Option<f64> {
    let n = rnw_pts.len();
    let mut close = Vec::new();
    for &(px, py) in rnw_pts {
        let d = pt_to_poly_m(px, py, map_poly);
        if d < 30.0 {
            close.push(d);
        }
    }
    if close.len() < 2 || 5 * close.len() < 4 * n {
        return None;
    }
    let s: f64 = close.iter().sum::<f64>() / close.len() as f64;
    if s <= 20.0 {
        Some(s)
    } else {
        None
    }
}

fn fmt_i(v: Option<i64>) -> String {
    match v {
        Some(x) => x.to_string(),
        None => "None".to_string(),
    }
}

// ------------------------------------------------------------- output writer

fn xml_esc(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

fn emit_element(el: &OsmEl, tags_override: Option<&Vec<(String, String)>>, out: &mut impl Write) {
    let mut s = String::new();
    s.push('<');
    s.push_str(&el.tag);
    for (k, v) in &el.attrs {
        s.push(' ');
        s.push_str(k);
        s.push_str("=\"");
        xml_esc(v, &mut s);
        s.push('"');
    }
    match el.tag.as_str() {
        "way" => {
            s.push_str(">\n");
            for r in &el.nds {
                s.push_str("<nd ref=\"");
                s.push_str(r);
                s.push_str("\"/>");
            }
            s.push('\n');
            let tags = tags_override.unwrap_or(&el.tags);
            for (k, v) in tags {
                s.push_str("<tag k=\"");
                xml_esc(k, &mut s);
                s.push_str("\" v=\"");
                xml_esc(v, &mut s);
                s.push_str("\"/>");
            }
            s.push_str("</way>\n");
        }
        _ => {
            s.push_str("/>\n");
        }
    }
    out.write_all(s.as_bytes()).unwrap();
}

// ---------------------------------------------------------------------- main

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: rnw_join_rs <rnw_roads.jsonl> <map.osm> <out.osm>");
        exit(1);
    }
    let (roads, grid) = load_rnw(&args[1]);
    eprintln!("rnw roads indexed: {}", roads.len());

    let data = match fs::read(&args[2]) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read {}: {}", args[2], e);
            exit(1);
        }
    };

    // pass 1: node coordinates + road ways (id -> refs, tags)
    let mut stream = match OsmStream::new(&data) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        }
    };
    let mut nodes: HashMap<i64, (f64, f64)> = HashMap::new();
    let mut road_ways: Vec<(i64, (Vec<i64>, Vec<(String, String)>))> = Vec::new();
    while let Some(el) = stream.next_element() {
        match el.tag.as_str() {
            "node" => {
                // stored as (lon, lat) to match the RNW jsonl point order
                nodes.insert(attr_i64(&el, "id"), (attr_f64(&el, "lon"), attr_f64(&el, "lat")));
            }
            "way" => {
                let mut tags: Vec<(String, String)> = Vec::new();
                for (k, v) in &el.tags {
                    match tags.iter_mut().find(|(kk, _)| kk == k) {
                        Some(e) => e.1 = v.clone(),
                        None => tags.push((k.clone(), v.clone())),
                    }
                }
                if tags.iter().any(|(k, v)| k == "tm:layer" && v == "road") {
                    let id = attr_i64(&el, "id");
                    let refs: Vec<i64> = el.nds.iter().map(|r| r.parse().unwrap()).collect();
                    road_ways.push((id, (refs, tags)));
                }
            }
            _ => {}
        }
    }
    eprintln!(
        "osm nodes: {}, road ways: {}",
        nodes.len(),
        road_ways.len()
    );

    // matching
    let mut updates: HashMap<i64, Vec<(String, String)>> = HashMap::new();
    let (mut enriched, mut cross_ok, mut cross_tot, mut with_highway) = (0u64, 0u64, 0u64, 0u64);
    for &(wid, ref rw) in &road_ways {
        let (refs, tags) = rw;
        let coords: Option<Vec<(f64, f64)>> = if refs.iter().all(|r| nodes.contains_key(r)) {
            Some(refs.iter().map(|r| nodes[r]).collect())
        } else {
            None
        };
        let Some(coords) = coords else { continue };
        if coords.len() < 3 {
            continue;
        }
        let mut minx = f64::INFINITY;
        let mut maxx = f64::NEG_INFINITY;
        let mut miny = f64::INFINITY;
        let mut maxy = f64::NEG_INFINITY;
        for &(x, y) in &coords {
            if x < minx {
                minx = x;
            }
            if x > maxx {
                maxx = x;
            }
            if y < miny {
                miny = y;
            }
            if y > maxy {
                maxy = y;
            }
        }
        let mbbox = (minx, miny, maxx, maxy);
        let mapcells: HashSet<(i32, i32)>;
        let mut cands: Vec<u32> = Vec::new();
        let mut cand_seen: HashSet<u32> = HashSet::new();
        {
            let mut cells: Vec<(i32, i32)> = Vec::new();
            for gx in (mbbox.0 / CELL) as i32..=(mbbox.2 / CELL) as i32 {
                for gy in (mbbox.1 / CELL) as i32..=(mbbox.3 / CELL) as i32 {
                    cells.push((gx, gy));
                    if let Some(list) = grid.get(&(gx, gy)) {
                        for &i in list {
                            if cand_seen.insert(i) {
                                cands.push(i);
                            }
                        }
                    }
                }
            }
            mapcells = cells.into_iter().collect();
        }

        let mut comps: Vec<(f64, u32)> = Vec::new();
        for &i in &cands {
            let r = &roads[i as usize];
            if !(r.minx - 0.002 < mbbox.2
                && r.maxx + 0.002 > mbbox.0
                && r.miny - 0.002 < mbbox.3
                && r.maxy + 0.002 > mbbox.1)
            {
                continue;
            }
            let pts = &r.pts;
            // both ends must lie on the MAP line (cell check first, then exact)
            if !mapcells.contains(&((pts[0].0 / CELL) as i32, (pts[0].1 / CELL) as i32)) {
                continue;
            }
            let last = pts.len() - 1;
            if !mapcells
                .contains(&((pts[last].0 / CELL) as i32, (pts[last].1 / CELL) as i32))
            {
                continue;
            }
            if pt_to_poly_m(pts[0].0, pts[0].1, &coords) >= 30.0 {
                continue;
            }
            if pt_to_poly_m(pts[last].0, pts[last].1, &coords) >= 30.0 {
                continue;
            }
            if let Some(s) = component_score(pts, &coords) {
                comps.push((s, i));
            }
        }
        if comps.is_empty() {
            continue;
        }
        // best score per road id, keeping first-seen order for stable sorting
        let mut best_order: Vec<u32> = Vec::new();
        let mut best_score: HashMap<u32, f64> = HashMap::new();
        for (s, i) in &comps {
            match best_score.get(i) {
                Some(&cur) => {
                    if *s < cur {
                        best_score.insert(*i, *s);
                    }
                }
                None => {
                    best_order.push(*i);
                    best_score.insert(*i, *s);
                }
            }
        }
        best_order.sort_by(|a, b| {
            best_score[a]
                .partial_cmp(&best_score[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // total covered length by named components
        let mut names: Vec<String> = Vec::new();
        let mut attrs: Option<&RnwRoad> = None;
        let mut attrs_len = -1.0f64;
        for &i in &best_order {
            let r = &roads[i as usize];
            if let Some(nm) = &r.name {
                if !nm.is_empty() {
                    let up = nm.to_uppercase();
                    if !names.iter().any(|n| n.to_uppercase() == up) {
                        names.push(nm.clone());
                    }
                }
            }
            let mut plen = 0.0f64;
            for j in 0..r.pts.len() - 1 {
                plen += seg_len_m(
                    r.pts[j].0,
                    r.pts[j].1,
                    r.pts[j + 1].0,
                    r.pts[j + 1].1,
                );
            }
            if plen > attrs_len {
                attrs_len = plen;
                attrs = Some(r);
            }
        }
        let Some(attrs) = attrs else { continue };

        let mut new_tags = tags.clone();
        if !tags.iter().any(|(k, _)| k == "name") {
            if !names.is_empty() {
                new_tags.push(("name".to_string(), names[0].clone()));
                if names.len() > 1 {
                    new_tags.push((
                        "name:alt".to_string(),
                        names[1..].join("; "),
                    ));
                }
                enriched += 1;
            }
        } else {
            cross_tot += 1;
            let tname = tags.iter().find(|(k, _)| k == "name").unwrap();
            let tname_up = tname.1.to_uppercase();
            if names.iter().any(|n| n.to_uppercase() == tname_up) {
                cross_ok += 1;
            }
        }
        // class attributes + OSM highway, added to every matched road
        new_tags.push(("rn_class".to_string(), fmt_i(attrs.rc)));
        new_tags.push(("rn_netclass".to_string(), fmt_i(attrs.nc)));
        new_tags.push(("rn_roadtype".to_string(), fmt_i(attrs.rt)));
        new_tags.push(("rn_link".to_string(), fmt_i(attrs.link)));
        new_tags.push(("rn_sec".to_string(), fmt_i(attrs.sec)));
        new_tags.push(("rn_freeway".to_string(), fmt_i(attrs.fw)));
        if let (Some(rc), Some(nc)) = (attrs.rc, attrs.nc) {
            if let Some(dc) = display_class(rc, nc) {
                let hw = highway_tag(dc, attrs.link == Some(1));
                if !hw.is_empty() {
                    new_tags.push(("highway".to_string(), hw.to_string()));
                    with_highway += 1;
                }
            }
        }
        updates.insert(wid, new_tags);
    }

    // pass 2: stream-copy the file, replacing tags of updated road ways
    let mut stream = match OsmStream::new(&data) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}", e);
            exit(1);
        }
    };
    let file = match fs::File::create(&args[3]) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("cannot create {}: {}", args[3], e);
            exit(1);
        }
    };
    let mut out = BufWriter::new(file);
    out.write_all(b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n")
        .unwrap();
    out.write_all(format!("<osm version=\"0.6\" generator=\"{}\">\n", GENERATOR).as_bytes())
        .unwrap();
    while let Some(el) = stream.next_element() {
        if el.tag == "way" {
            let id = attr_i64(&el, "id");
            if let Some(tags) = updates.get(&id) {
                emit_element(&el, Some(tags), &mut out);
                continue;
            }
        }
        emit_element(&el, None, &mut out);
    }
    out.write_all(b"</osm>\n").unwrap();
    if let Err(e) = out.flush() {
        eprintln!("write error: {}", e);
        exit(1);
    }
    println!("unnamed roads enriched: {}", enriched);
    println!("roads with highway tag: {}", with_highway);
    println!("named cross-check: {}/{} agree", cross_ok, cross_tot);
}
