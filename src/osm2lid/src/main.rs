// osm2lid — OSM (PBF or XML) -> Bosch TravelMap LID address/POI databases (SQLite).
//
// Generates, for one content region:
//   * GLOB_POI.DAT  — the POI gazetteer (SQLite FTS3, table GLOBAL_POIS). Cities + POIs.
//   * DB_CITY.DAT   — the global addressable-city list (SQLite FTS3, GlobalCityList).
// Both match the exact schemas the car reads (LID_format.md §3 / §10.5) and are validated
// by the reader `lid2dump` (round-trip). Coordinates are PAU = deg * 2^31 / 180.
//
// Scope note: writes the SQLite half (cities + POIs) AND the street name-list half
// (`LID20006.DAT`, the ASF columnar trie, §12) built by `src/lid_format::encode`. House-number
// GenAttr (+20000) and point-address (`PA_%05u`) tables are still pending. The street name-list is
// validated by the reader `lid2dump` (full OSM -> LID -> dump round-trip, position- and name-exact).
//
// Usage: osm2lid <in.osm.pbf|in.osm> -o OUTDIR [--region POL] [--lang 22] [--bbox W,S,E,N] [--no-poi]

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::Path;
use std::process::exit;

use rusqlite::{params, Connection};

const PAU: f64 = (1i64 << 31) as f64 / 180.0;

// regionIdent (0x400 | codeId, profile 1) per RNW region code — see doc/region_ident.tsv.
const REGION_IDENT: [(&str, u16); 17] = [
    ("DEU", 0x401), ("POL", 0x402), ("FRM", 0x403), ("BNL", 0x404), ("GRC", 0x406),
    ("TUR", 0x407), ("ACL", 0x408), ("IBE", 0x409), ("MLC", 0x40a), ("ISV", 0x40b),
    ("GBI", 0x40c), ("SCA", 0x40d), ("CHS", 0x40e), ("ELL", 0x411), ("INT", 0x412),
    ("EAD", 0x416), ("EEU", 0x42a),
];

// GLOB_POI category for a generated city/town row. (Best-effort; refine from POI_MAPPING.DAT.)
const CAT_CITY: i64 = 63; // == GlobalCityList.CAT_ID city marker (LID_format §10.5)
const CAT_POI: i64 = 0;   // generic POI bucket when no specific category is known

fn deg2pau(d: f64) -> i64 {
    (d * PAU) as i64
}

struct Data {
    nodes: HashMap<i64, (i64, i64)>,
    places: Vec<(String, (i64, i64), String)>, // (name, coord, place-kind)
    pois: Vec<(String, (i64, i64))>,           // (name, coord)
    streets: Vec<(String, Vec<i64>)>,          // (name, node refs) — Phase-2 (not emitted)
}

impl Data {
    fn new() -> Data {
        Data { nodes: HashMap::new(), places: Vec::new(), pois: Vec::new(), streets: Vec::new() }
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut input: Option<String> = None;
    let mut out: Option<String> = None;
    let mut region = "POL".to_string();
    let mut lang: i64 = 22;
    let mut bbox: Option<(f64, f64, f64, f64)> = None;
    let mut no_poi = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => { i += 1; out = args.get(i).cloned(); }
            "--region" => { i += 1; region = args.get(i).cloned().unwrap_or(region); }
            "--lang" => { i += 1; lang = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(lang); }
            "--bbox" => { i += 1; bbox = args.get(i).and_then(|s| parse_bbox(s)); }
            "--no-poi" => no_poi = true,
            "-h" | "--help" => { usage(); exit(0); }
            other => input = Some(other.to_string()),
        }
        i += 1;
    }
    let (Some(input), Some(out)) = (input, out) else { usage(); exit(1) };

    let region_id = match REGION_IDENT.iter().find(|(c, _)| c.eq_ignore_ascii_case(&region)) {
        Some((_, id)) => *id,
        None => {
            eprintln!("unknown region '{region}'; known: {}", REGION_IDENT.iter().map(|(c,_)| *c).collect::<Vec<_>>().join(","));
            exit(1);
        }
    };

    let mut data = Data::new();
    if input.ends_with(".pbf") || input.ends_with(".osm.pbf") || input.ends_with(".pbfs") {
        parse_pbf(&input, &mut data);
    } else {
        parse_osm_xml(&input, &mut data);
    }
    eprintln!(
        "parsed: {} nodes, {} places, {} POIs, {} street-name ways",
        data.nodes.len(), data.places.len(), data.pois.len(), data.streets.len()
    );

    let outdir = Path::new(&out);
    fs::create_dir_all(outdir).unwrap_or_else(|e| { eprintln!("mkdir {out}: {e}"); exit(1); });

    let npoi = write_glob_poi(&outdir.join("GLOB_POI.DAT"), &data, region_id, lang, bbox, !no_poi);
    let ncity = write_db_city(&outdir.join("DB_CITY.DAT"), &data, region_id, bbox);
    let ncit = write_cities(&outdir.join("LID20000.DAT"), &data, bbox);
    let nst = write_streets(&outdir.join("LID20006.DAT"), &data, bbox);

    eprintln!(
        "wrote {}/GLOB_POI.DAT ({}), DB_CITY.DAT ({}), LID20000.DAT ({} cities), LID20006.DAT ({} streets), REGION_ID=0x{:03x} ({})",
        out, npoi, ncity, ncit, nst, region_id, region.to_uppercase()
    );
    eprintln!("NOTE: house-number GenAttr (+20000) and point-address (PA) tables are not generated yet — see LID_format.md §12.");
}

fn usage() {
    eprintln!(
        "Usage: osm2lid <in.osm.pbf|in.osm> -o OUTDIR [opts]\n\
        \t--region POL   content region code (sets REGION_ID = regionIdent)\n\
        \t--lang 22      LANG_IDX tag written on GLOB_POI rows (language index)\n\
        \t--bbox W,S,E,N keep only entries within this lon/lat box\n\
        \t--no-poi       skip amenity/shop/tourism POIs (cities only)"
    );
}

// --------------------------------------------------------------------------- OSM parse

fn parse_pbf(path: &str, d: &mut Data) {
    use pbf_craft::models::{Element, Tag as PbfTag};
    use pbf_craft::readers::PbfReader;
    fn nd_to_deg(nd: i64) -> f64 {
        if nd < 0 { return -nd_to_deg(-nd); }
        let whole = nd / 1_000_000_000;
        let frac = nd % 1_000_000_000;
        format!("{}.{:09}", whole, frac).parse::<f64>().unwrap_or_else(|_| nd as f64 / 1e9)
    }
    let mut reader = PbfReader::from_path(path).unwrap_or_else(|e| panic!("open pbf {path}: {e}"));
    reader
        .read(|_h, el| {
            let Some(el) = el else { return };
            match el {
                Element::Node(n) => {
                    if !n.visible { return; }
                    let c = (deg2pau(nd_to_deg(n.longitude)), deg2pau(nd_to_deg(n.latitude)));
                    d.nodes.insert(n.id, c);
                    let tags: HashMap<String, String> =
                        n.tags.iter().map(|t: &PbfTag| (t.key.clone(), t.value.clone())).collect();
                    tag_node(d, c, &tags);
                }
                Element::Way(w) => {
                    if !w.visible { return; }
                    let ids: Vec<i64> = w.way_nodes.iter().map(|wn| wn.id).collect();
                    let tags: HashMap<String, String> =
                        w.tags.iter().map(|t: &PbfTag| (t.key.clone(), t.value.clone())).collect();
                    tag_way(d, ids, &tags);
                }
                Element::Relation(_) => {}
            }
        })
        .unwrap_or_else(|e| panic!("read pbf {path}: {e}"));
}

fn parse_osm_xml(path: &str, d: &mut Data) {
    use quick_xml::events::Event;
    use std::io::BufReader;
    let file = fs::File::open(path).expect("open osm");
    let mut reader = quick_xml::Reader::from_reader(BufReader::new(file));
    let mut buf = Vec::with_capacity(1 << 16);
    let mut node: Option<(i64, (i64, i64))> = None;
    let mut node_tags: HashMap<String, String> = HashMap::new();
    let mut way_ids: Option<Vec<i64>> = None;
    let mut way_tags: HashMap<String, String> = HashMap::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                "node" => { node = Some((0, (0, 0))); node_tags.clear(); for a in e.attributes().flatten() { match a.key.as_ref() { "id" => node.as_mut().unwrap().0 = str_of(&a).parse().unwrap_or(0), "lon" => node.as_mut().unwrap().1.0 = deg2pau(str_of(&a).parse().unwrap_or(0.0)), "lat" => node.as_mut().unwrap().1.1 = deg2pau(str_of(&a).parse().unwrap_or(0.0)), _ => {} } } }
                "way" => { way_ids = Some(Vec::new()); way_tags.clear(); }
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "node" => { let (mut id, mut lo, mut la) = (0i64, 0i64, 0i64); for a in e.attributes().flatten() { match a.key.as_ref() { "id" => id = str_of(&a).parse().unwrap_or(0), "lon" => lo = deg2pau(str_of(&a).parse().unwrap_or(0.0)), "lat" => la = deg2pau(str_of(&a).parse().unwrap_or(0.0)), _ => {} } } d.nodes.insert(id, (lo, la)); }
                "nd" => { if let Some(ids) = &mut way_ids { for a in e.attributes().flatten() { if a.key.as_ref() == "ref" { if let Ok(v) = str_of(&a).parse::<i64>() { ids.push(v); } } } } }
                "tag" => { let (mut k, mut v) = (String::new(), String::new()); for a in e.attributes().flatten() { let s = str_of(&a); match a.key.as_ref() { "k" => k = s, "v" => v = s, _ => {} } } if node.is_some() { node_tags.insert(k, v); } else if way_ids.is_some() { way_tags.insert(k, v); } }
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                "node" => { if let Some((id, c)) = node.take() { d.nodes.insert(id, c); tag_node(d, c, &node_tags); node_tags.clear(); } }
                "way" => { if let Some(ids) = way_ids.take() { let t = std::mem::take(&mut way_tags); tag_way(d, ids, &t); } }
                _ => {}
            },
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
}

fn str_of(a: &quick_xml::events::attributes::Attribute) -> String {
    a.unescape_value().map(|s| s.into_owned()).unwrap_or_else(|_| a.value.as_ref().to_string())
}

fn tag_node(d: &mut Data, c: (i64, i64), t: &HashMap<String, String>) {
    if let (Some(name), Some(kind)) = (t.get("name"), t.get("place")) {
        if is_city_kind(kind) {
            d.places.push((name.clone(), c, kind.clone()));
            return;
        }
    }
    // amenity / shop / tourism POIs with a name
    if t.contains_key("amenity") || t.contains_key("shop") || t.contains_key("tourism") || t.contains_key("craft") {
        if let Some(name) = t.get("name") {
            d.pois.push((name.clone(), c));
        }
    }
}

fn tag_way(d: &mut Data, ids: Vec<i64>, t: &HashMap<String, String>) {
    if ids.len() < 2 { return; }
    // Street name-lists are Phase 2; we still parse them for reporting.
    if t.contains_key("highway") {
        if let Some(name) = t.get("name") {
            d.streets.push((name.clone(), ids));
        }
    }
}

fn is_city_kind(k: &str) -> bool {
    matches!(k, "city" | "town" | "large_town" | "small_city" | "village" | "suburb" | "hamlet")
}

// --------------------------------------------------------------------------- writers

fn write_glob_poi(path: &Path, d: &Data, region_id: u16, lang: i64, bbox: Option<(f64, f64, f64, f64)>, include_pois: bool) -> usize {
    let _ = fs::remove_file(path);
    let db = Connection::open(path).unwrap_or_else(|e| { eprintln!("open {path:?}: {e}"); exit(1); });
    db.execute_batch(
        "CREATE VIRTUAL TABLE GLOBAL_POIS USING FTS3 (\
            IDX NUMBER NOT NULL, NAMENORM VARCHAR(2000) NOT NULL, NAME VARCHAR(2000), \
            LANG_IDX NUMBER NOT NULL, ORIGINAL_IDX NUMBER, LONGITUDE NUMBER NOT NULL, \
            LATITUDE NUMBER NOT NULL, CAT_ID NUMBER NOT NULL, REGION_ID NUMBER NOT NULL, CONTROL NUMBER NOT NULL);"
    ).expect("create GLOBAL_POIS (FTS3 available in bundled SQLite)");
    let mut idx = 0i64;
    {
        let tx = db.unchecked_transaction().unwrap();
        let mut ins = tx.prepare(
            "INSERT INTO GLOBAL_POIS (IDX,NAMENORM,NAME,LANG_IDX,ORIGINAL_IDX,LONGITUDE,LATITUDE,CAT_ID,REGION_ID,CONTROL) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)"
        ).unwrap();
        let mut row = |name: &str, c: (i64, i64), cat: i64| {
            if !in_bbox(c, bbox) { return; }
            idx += 1;
            let norm = fold(name);
            ins.execute(params![idx, norm, name, lang, idx, c.0, c.1, cat, region_id as i64, 1i64]).unwrap();
        };
        for (name, c, _kind) in &d.places { row(name, *c, CAT_CITY); }
        if include_pois { for (name, c) in &d.pois { row(name, *c, CAT_POI); } }
        drop(ins);
        tx.commit().unwrap();
    }
    idx as usize
}

fn write_db_city(path: &Path, d: &Data, region_id: u16, bbox: Option<(f64, f64, f64, f64)>) -> usize {
    let _ = fs::remove_file(path);
    let db = Connection::open(path).unwrap_or_else(|e| { eprintln!("open {path:?}: {e}"); exit(1); });
    // Exact columns the car SELECTs (bGetGlobalCityList/bGetCityByID): NameNorm is the FTS key.
    db.execute_batch(
        "CREATE VIRTUAL TABLE GlobalCityList USING FTS3 (\
            ID NUMBER NOT NULL, Name VARCHAR, NameNorm VARCHAR NOT NULL, \
            Longitude NUMBER NOT NULL, Latitude NUMBER NOT NULL, Province_ID NUMBER, CAT_ID NUMBER);"
    ).expect("create GlobalCityList (FTS3 available in bundled SQLite)");
    let _ = region_id; // Province_ID left 0 (no admin mapping yet)
    let mut id = 0i64;
    {
        let tx = db.unchecked_transaction().unwrap();
        let mut ins = tx.prepare(
            "INSERT INTO GlobalCityList (ID,Name,NameNorm,Longitude,Latitude,Province_ID,CAT_ID) VALUES (?1,?2,?3,?4,?5,?6,?7)"
        ).unwrap();
        for (name, c, _k) in &d.places {
            if !in_bbox(*c, bbox) { continue; }
            id += 1;
            ins.execute(params![id, name, fold(name), c.0, c.1, 0i64, CAT_CITY]).unwrap();
        }
        drop(ins);
        tx.commit().unwrap();
    }
    id as usize
}

/// Emit a street name-list (`LID2*` ASF columnar trie, §12) from the parsed highway `name` ways.
/// Each unique street name becomes one element at its way centroid (absolute PAU; block origin 0).
/// Written bytes are exactly the format `lid_format::read` (and the car's `NLProcessor`) parses.
fn write_streets(path: &Path, d: &Data, bbox: Option<(f64, f64, f64, f64)>) -> usize {
    use lid_format::NameEntry;
    let mut seen: HashSet<String> = HashSet::new();
    let mut entries: Vec<NameEntry> = Vec::new();
    for (name, refs) in &d.streets {
        let nm = name.trim();
        if nm.is_empty() || nm.contains('\0') { continue }
        let (mut sx, mut sy, mut k) = (0i64, 0i64, 0i64);
        for &r in refs {
            if let Some(c) = d.nodes.get(&r) { sx += c.0; sy += c.1; k += 1 }
        }
        if k == 0 { continue }
        let (cx, cy) = (sx / k, sy / k);
        if !in_bbox((cx, cy), bbox) { continue }
        if !seen.insert(nm.to_string()) { continue } // dedup by name (keep first occurrence)
        entries.push(NameEntry { label: nm.to_string(), x_pau: cx as i32, y_pau: cy as i32 });
    }
    let bytes = lid_format::encode(&entries);
    if fs::write(path, &bytes).is_err() { eprintln!("write {path:?} failed"); return 0 }
    entries.len()
}

/// Emit a city/locality name-list (`LID20000.DAT`, the settlement ASF name-list) from the parsed
/// `place=` nodes. One element per unique place name at its coordinate (absolute PAU).
fn write_cities(path: &Path, d: &Data, bbox: Option<(f64, f64, f64, f64)>) -> usize {
    use lid_format::NameEntry;
    let mut seen: HashSet<String> = HashSet::new();
    let mut entries: Vec<NameEntry> = Vec::new();
    for (name, c, _kind) in &d.places {
        let nm = name.trim();
        if nm.is_empty() || nm.contains('\0') || !in_bbox(*c, bbox) { continue }
        if !seen.insert(nm.to_string()) { continue }
        entries.push(NameEntry { label: nm.to_string(), x_pau: c.0 as i32, y_pau: c.1 as i32 });
    }
    let bytes = lid_format::encode(&entries);
    if fs::write(path, &bytes).is_err() { eprintln!("write {path:?} failed"); return 0 }
    entries.len()
}

fn in_bbox(c: (i64, i64), bbox: Option<(f64, f64, f64, f64)>) -> bool {
    match bbox {
        None => true,
        Some((w, s, e, n)) => {
            c.0 >= deg2pau(w) && c.0 <= deg2pau(e) && c.1 >= deg2pau(s) && c.1 <= deg2pau(n)
        }
    }
}

fn parse_bbox(s: &str) -> Option<(f64, f64, f64, f64)> {
    let p: Vec<f64> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if p.len() == 4 { Some((p[0], p[1], p[2], p[3])) } else { None }
}

/// ASCII-fold a name for the FTS NAMENORM column: uppercase, strip accents, keep [A-Z0-9-. ]
fn fold(s: &str) -> String {
    s.to_uppercase()
        .chars()
        .map(|c| match c {
            'Ą' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
            'Ć' | 'Ç' | 'Č' => 'C',
            'Ę' | 'È' | 'É' | 'Ê' | 'Ë' => 'E',
            'Ł' => 'L',
            'Ń' | 'Ñ' => 'N',
            'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => 'O',
            'Ś' | 'Š' | 'Ș' => 'S',
            'Ź' | 'Ż' | 'Ž' => 'Z',
            'Ü' | 'Û' | 'Ù' | 'ÿ' => 'U',
            'Ý' => 'Y',
            c if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
            c => c,
        })
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ' ') { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
