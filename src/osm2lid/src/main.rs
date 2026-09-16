// osm2lid — OSM (PBF or XML) -> Bosch TravelMap LID address/POI databases (SQLite).
//
// Generates, for one content region:
//   * GLOB_POI.DAT  — the POI gazetteer (SQLite FTS3, table GLOBAL_POIS). Cities + POIs.
//   * DB_CITY.DAT   — the global addressable-city list (SQLite FTS3, GlobalCityList).
// Both match the exact schemas the car reads (LID_format.md §3 / §10.5) and are validated
// by the reader `lid2dump` (round-trip). Coordinates are PAU = deg * 2^31 / 180.
//
// Scope note: writes the SQLite half (cities + POIs), the street + city name-lists
// (`LID20006.DAT` / `LID20000.DAT`, the ASF columnar trie, §12) built by `src/lid_format::encode`, and the
// house-number GenAttr half (`LID40006.DAT`, §11.6) built by `src/lid_format::write` from OSM
// `addr:housenumber` objects joined to their `addr:street`. Point-address (`PA_%05u`) tables and crossing
// (+10000) files are still pending. The street/city name-lists are validated by `lid2dump`
// (full OSM -> LID -> dump round-trip, position- and name-exact); the GenAttr writer is validated offline by
// `lid_format`'s `gen_attr_*` tests (byte-exact rebuild oracle + a device read-model round-trip).
//
// Usage: osm2lid <in.osm.pbf|in.osm> -o OUTDIR [--region POL] [--lang 22] [--bbox W,S,E,N] [--no-poi] [--no-genattr]

use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::path::Path;
use std::process::exit;

use rusqlite::{params, Connection};

const PAU: f64 = (1i64 << 31) as f64 / 180.0;

// regionIdent (0x400 | codeId, profile 1) per RNW region code — see doc/region_ident.tsv.
const REGION_IDENT: [(&str, u16); 17] = [
    ("DEU", 0x401),
    ("POL", 0x402),
    ("FRM", 0x403),
    ("BNL", 0x404),
    ("GRC", 0x406),
    ("TUR", 0x407),
    ("ACL", 0x408),
    ("IBE", 0x409),
    ("MLC", 0x40a),
    ("ISV", 0x40b),
    ("GBI", 0x40c),
    ("SCA", 0x40d),
    ("CHS", 0x40e),
    ("ELL", 0x411),
    ("INT", 0x412),
    ("EAD", 0x416),
    ("EEU", 0x42a),
];

// GLOB_POI category for a generated city/town row. (Best-effort; refine from POI_MAPPING.DAT.)
const CAT_CITY: i64 = 63; // == GlobalCityList.CAT_ID city marker (LID_format §10.5)
const CAT_POI: i64 = 0; // generic POI bucket when no specific category is known

fn deg2pau(d: f64) -> i64 {
    (d * PAU) as i64
}

struct Data {
    nodes: HashMap<i64, (i64, i64)>,
    places: Vec<(String, (i64, i64), String)>, // (name, coord, place-kind)
    pois: Vec<(String, (i64, i64))>,           // (name, coord)
    streets: Vec<(String, Vec<i64>)>,          // (name, node refs) — Phase-2 (not emitted)
    addrs: Vec<(String, (i64, i64), String)>,  // (housenumber, coord, street) — GenAttr source
}

impl Data {
    fn new() -> Data {
        Data {
            nodes: HashMap::new(),
            places: Vec::new(),
            pois: Vec::new(),
            streets: Vec::new(),
            addrs: Vec::new(),
        }
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
    let mut no_genattr = false;
    let mut no_pa = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                i += 1;
                out = args.get(i).cloned();
            }
            "--region" => {
                i += 1;
                region = args.get(i).cloned().unwrap_or(region);
            }
            "--lang" => {
                i += 1;
                lang = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(lang);
            }
            "--bbox" => {
                i += 1;
                bbox = args.get(i).and_then(|s| parse_bbox(s));
            }
            "--no-poi" => no_poi = true,
            "--no-genattr" => no_genattr = true,
            "--no-pa" => no_pa = true,
            "-h" | "--help" => {
                usage();
                exit(0);
            }
            other => input = Some(other.to_string()),
        }
        i += 1;
    }
    let (Some(input), Some(out)) = (input, out) else {
        usage();
        exit(1)
    };

    let region_id = match REGION_IDENT
        .iter()
        .find(|(c, _)| c.eq_ignore_ascii_case(&region))
    {
        Some((_, id)) => *id,
        None => {
            eprintln!(
                "unknown region '{region}'; known: {}",
                REGION_IDENT
                    .iter()
                    .map(|(c, _)| *c)
                    .collect::<Vec<_>>()
                    .join(",")
            );
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
        "parsed: {} nodes, {} places, {} POIs, {} street-name ways, {} addr:housenumber objects",
        data.nodes.len(),
        data.places.len(),
        data.pois.len(),
        data.streets.len(),
        data.addrs.len()
    );

    let outdir = Path::new(&out);
    fs::create_dir_all(outdir).unwrap_or_else(|e| {
        eprintln!("mkdir {out}: {e}");
        exit(1);
    });

    let npoi = write_glob_poi(
        &outdir.join("GLOB_POI.DAT"),
        &data,
        region_id,
        lang,
        bbox,
        !no_poi,
    );
    let ncity = write_db_city(&outdir.join("DB_CITY.DAT"), &data, region_id, bbox);
    let (ncit, city_idx) = write_cities(&outdir.join("LID20001.DAT"), &data, bbox, region_id, 2);
    let (st_entries, city_of) = collect_street_entries(&data, bbox);    let (nst, street_idx) =
        write_name_list_idx(&outdir.join("LID20006.DAT"), &st_entries, region_id, 3);
    let nrel = write_street_city_rel(
        &outdir.join("REL00001.DAT"),
        &st_entries,
        &city_of,
        &street_idx,
        &city_idx,
    );
    let npa = if no_pa {
        0
    } else {
        write_pa(
            &outdir.join("PA_20006.DAT"),
            &data,
            bbox,
            &st_entries,
            &street_idx,
            nst,
            region_id,
        )
    };
    let naddr = if no_genattr {
        0
    } else {
        write_gen_attr(
            &outdir.join("LID40006.DAT"),
            &data,
            bbox,
            &street_idx,
            nst,
            region_id,
        )
    };

    eprintln!(
        "wrote {}/GLOB_POI.DAT ({}), DB_CITY.DAT ({}), LID20001.DAT ({} cities), LID20006.DAT ({} streets), REL00001.DAT ({} street→city pairs), LID40006.DAT ({} house numbers), PA_20006.DAT ({} access points), REGION_ID=0x{:03x} ({})",
        out, npoi, ncity, ncit, nst, nrel, naddr, npa, region_id, region.to_uppercase()
    );
    eprintln!("NOTE: ship the stock META0000.DAT unchanged — its relation table entry #1 is (2↔3), which is what REL00001.DAT carries.");
    eprintln!("NOTE: crossing (+10000) tables are not generated yet — see LID_format.md §12. PA file card-test pending (no stock PA sample exists on any card).");
}

fn usage() {
    eprintln!(
        "Usage: osm2lid <in.osm.pbf|in.osm> -o OUTDIR [opts]\n\
        \t--region POL   content region code (sets REGION_ID = regionIdent)\n\
        \t--lang 22      LANG_IDX tag written on GLOB_POI rows (language index)\n\
        \t--bbox W,S,E,N keep only entries within this lon/lat box\n\
        \t--no-poi       skip amenity/shop/tourism POIs (cities only)\n\
        \t--no-genattr   skip the LID40006.DAT house-number (GenAttr +20000) file\n\
        \t--no-pa        skip the PA_20006.DAT point-access-point file"
    );
}

// --------------------------------------------------------------------------- OSM parse

// PBF dense-nodes-first ordering is *block*-scoped (primitives per ~16k nodes), not file-global, so a way's
// nodes may not have streamed yet. Buffer (coordinate + relevant tags) per node, and (refs + relevant tags)
// per interesting way, resolve refs after the stream closes; tag_node/tag_way then see every way complete.
fn parse_pbf(path: &str, d: &mut Data) {
    use pbf_craft::models::Element;
    use pbf_craft::readers::PbfReader;
    fn nd_to_deg(nd: i64) -> f64 {
        if nd < 0 {
            return -nd_to_deg(-nd);
        }
        let whole = nd / 1_000_000_000;
        let frac = nd % 1_000_000_000;
        format!("{}.{:09}", whole, frac)
            .parse::<f64>()
            .unwrap_or_else(|_| nd as f64 / 1e9)
    }
    const REL: [&str; 8] = [
        "name",
        "place",
        "amenity",
        "shop",
        "tourism",
        "craft",
        "addr:housenumber",
        "addr:street",
    ];
    let mut pnodes: Vec<((i64, i64), HashMap<String, String>)> = Vec::new();
    let mut pways: Vec<(Vec<i64>, HashMap<String, String>)> = Vec::new();
    let mut reader = PbfReader::from_path(path).unwrap_or_else(|e| panic!("open pbf {path}: {e}"));
    reader
        .read(|_h, el| {
            let Some(el) = el else { return };
            match el {
                Element::Node(n) => {
                    if !n.visible {
                        return;
                    }
                    let c = (
                        deg2pau(nd_to_deg(n.longitude)),
                        deg2pau(nd_to_deg(n.latitude)),
                    );
                    d.nodes.insert(n.id, c);
                    let mut tags: HashMap<String, String> = HashMap::new();
                    for t in &n.tags {
                        let k: &str = &t.key;
                        let k = if k == "addr:place" { "addr:street" } else { k }; // addr:place fallback
                        if REL.contains(&k) {
                            tags.insert(k.to_string(), t.value.clone());
                        }
                    }
                    if !tags.is_empty() {
                        pnodes.push((c, tags));
                    }
                }
                Element::Way(w) => {
                    if !w.visible {
                        return;
                    }
                    let mut tags: HashMap<String, String> = HashMap::new();
                    for t in w.tags.iter() {
                        let k: &str = &t.key;
                        let k = if k == "addr:place" { "addr:street" } else { k };
                        if k == "addr:housenumber"
                            || k == "highway"
                            || k == "name"
                            || (k == "addr:street"
                                && w.tags.iter().any(|t2| t2.key == "addr:housenumber"))
                        {
                            tags.insert(k.to_string(), t.value.clone());
                        }
                    }
                    let street = tags.contains_key("highway") && tags.contains_key("name");
                    let addr =
                        tags.contains_key("addr:housenumber") && tags.contains_key("addr:street");
                    if street || addr {
                        pways.push((w.way_nodes.iter().map(|wn| wn.id).collect(), tags));
                    }
                }
                Element::Relation(_) => {}
            }
        })
        .unwrap_or_else(|e| panic!("read pbf {path}: {e}"));
    for (c, tags) in pnodes {
        tag_node(d, c, &tags);
    }
    for (ids, tags) in pways {
        tag_way(d, ids, &tags);
    }
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
                "node" => {
                    node = Some((0, (0, 0)));
                    node_tags.clear();
                    for a in e.attributes().flatten() {
                        match a.key.as_ref() {
                            "id" => node.as_mut().unwrap().0 = str_of(&a).parse().unwrap_or(0),
                            "lon" => {
                                node.as_mut().unwrap().1 .0 =
                                    deg2pau(str_of(&a).parse().unwrap_or(0.0))
                            }
                            "lat" => {
                                node.as_mut().unwrap().1 .1 =
                                    deg2pau(str_of(&a).parse().unwrap_or(0.0))
                            }
                            _ => {}
                        }
                    }
                }
                "way" => {
                    way_ids = Some(Vec::new());
                    way_tags.clear();
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "node" => {
                    let (mut id, mut lo, mut la) = (0i64, 0i64, 0i64);
                    for a in e.attributes().flatten() {
                        match a.key.as_ref() {
                            "id" => id = str_of(&a).parse().unwrap_or(0),
                            "lon" => lo = deg2pau(str_of(&a).parse().unwrap_or(0.0)),
                            "lat" => la = deg2pau(str_of(&a).parse().unwrap_or(0.0)),
                            _ => {}
                        }
                    }
                    d.nodes.insert(id, (lo, la));
                }
                "nd" => {
                    if let Some(ids) = &mut way_ids {
                        for a in e.attributes().flatten() {
                            if a.key.as_ref() == "ref" {
                                if let Ok(v) = str_of(&a).parse::<i64>() {
                                    ids.push(v);
                                }
                            }
                        }
                    }
                }
                "tag" => {
                    let (mut k, mut v) = (String::new(), String::new());
                    for a in e.attributes().flatten() {
                        let s = str_of(&a);
                        match a.key.as_ref() {
                            "k" => k = s,
                            "v" => v = s,
                            _ => {}
                        }
                    }
                    if node.is_some() {
                        node_tags.insert(k, v);
                    } else if way_ids.is_some() {
                        way_tags.insert(k, v);
                    }
                }
                _ => {}
            },
            Ok(Event::End(e)) => match e.name().as_ref() {
                "node" => {
                    if let Some((id, c)) = node.take() {
                        d.nodes.insert(id, c);
                        tag_node(d, c, &node_tags);
                        node_tags.clear();
                    }
                }
                "way" => {
                    if let Some(ids) = way_ids.take() {
                        let t = std::mem::take(&mut way_tags);
                        tag_way(d, ids, &t);
                    }
                }
                _ => {}
            },
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
}

fn str_of(a: &quick_xml::events::attributes::Attribute) -> String {
    a.unescape_value()
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| a.value.as_ref().to_string())
}

fn tag_node(d: &mut Data, c: (i64, i64), t: &HashMap<String, String>) {
    if let (Some(name), Some(kind)) = (t.get("name"), t.get("place")) {
        if is_city_kind(kind) {
            d.places.push((name.clone(), c, kind.clone()));
            return;
        }
    }
    // amenity / shop / tourism POIs with a name
    if t.contains_key("amenity")
        || t.contains_key("shop")
        || t.contains_key("tourism")
        || t.contains_key("craft")
    {
        if let Some(name) = t.get("name") {
            d.pois.push((name.clone(), c));
        }
    }
    if let Some(num) = t.get("addr:housenumber") {
        if let Some(street) = t.get("addr:street").or_else(|| t.get("addr:place")) {
            d.addrs.push((num.clone(), c, street.clone()));
        }
    }
}

fn tag_way(d: &mut Data, ids: Vec<i64>, t: &HashMap<String, String>) {
    if ids.len() < 2 {
        return;
    }
    if t.contains_key("highway") {
        if let Some(name) = t.get("name") {
            d.streets.push((name.clone(), ids.clone()));
        }
    }
    // address on a building/entrance way: street from tags, coordinate = node centroid
    if let (Some(num), Some(street)) = (
        t.get("addr:housenumber"),
        t.get("addr:street").or_else(|| t.get("addr:place")),
    ) {
        let (mut sx, mut sy, mut k) = (0i64, 0i64, 0i64);
        for &r in &ids {
            if let Some(c) = d.nodes.get(&r) {
                sx += c.0;
                sy += c.1;
                k += 1
            }
        }
        if k > 0 {
            d.addrs
                .push((num.clone(), (sx / k, sy / k), street.clone()));
        }
    }
}

fn is_city_kind(k: &str) -> bool {
    matches!(
        k,
        "city" | "town" | "large_town" | "small_city" | "village" | "suburb" | "hamlet"
    )
}

// --------------------------------------------------------------------------- writers

fn write_glob_poi(
    path: &Path,
    d: &Data,
    region_id: u16,
    lang: i64,
    bbox: Option<(f64, f64, f64, f64)>,
    include_pois: bool,
) -> usize {
    let _ = fs::remove_file(path);
    let db = Connection::open(path).unwrap_or_else(|e| {
        eprintln!("open {path:?}: {e}");
        exit(1);
    });
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
            if !in_bbox(c, bbox) {
                return;
            }
            idx += 1;
            let norm = fold(name);
            ins.execute(params![
                idx,
                norm,
                name,
                lang,
                idx,
                c.0,
                c.1,
                cat,
                region_id as i64,
                1i64
            ])
            .unwrap();
        };
        for (name, c, _kind) in &d.places {
            row(name, *c, CAT_CITY);
        }
        if include_pois {
            for (name, c) in &d.pois {
                row(name, *c, CAT_POI);
            }
        }
        drop(ins);
        tx.commit().unwrap();
    }
    idx as usize
}

fn write_db_city(
    path: &Path,
    d: &Data,
    region_id: u16,
    bbox: Option<(f64, f64, f64, f64)>,
) -> usize {
    let _ = fs::remove_file(path);
    let db = Connection::open(path).unwrap_or_else(|e| {
        eprintln!("open {path:?}: {e}");
        exit(1);
    });
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
            if !in_bbox(*c, bbox) {
                continue;
            }
            id += 1;
            ins.execute(params![id, name, fold(name), c.0, c.1, 0i64, CAT_CITY])
                .unwrap();
        }
        drop(ins);
        tx.commit().unwrap();
    }
    id as usize
}

/// Collect the *unique* street name-list entries (dedup by name, way centroid) with the **owning city**
/// attached — the device stores street coordinates relative to the city's position (§12.5), so each street
/// gets the nearest `place=` node as its city anchor.
fn collect_street_entries(
    d: &Data,
    bbox: Option<(f64, f64, f64, f64)>,
) -> (Vec<lid_format::NameEntry>, Vec<Option<String>>) {
    use lid_format::NameEntry;
    let mut entries: Vec<NameEntry> = Vec::new();
    let mut city_of: Vec<Option<String>> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let places: Vec<(&String, (i64, i64))> = d.places.iter().map(|(n, c, _)| (n, *c)).collect();
    for (name, refs) in &d.streets {
        let nm = name.trim();
        if nm.is_empty() || nm.contains('\0') {
            continue;
        }
        let (mut sx, mut sy, mut k) = (0i64, 0i64, 0i64);
        for &r in refs {
            if let Some(c) = d.nodes.get(&r) {
                sx += c.0;
                sy += c.1;
                k += 1
            }
        }
        if k == 0 {
            continue;
        }
        let (cx, cy) = (sx / k, sy / k);
        if !in_bbox((cx, cy), bbox) {
            continue;
        }
        if !seen.insert(nm.to_string()) {
            continue;
        } // dedup by name (keep first occurrence)
        let city = nearest_place(&places, cx, cy);
        city_of.push(city.map(|(n, _)| n.clone()));
        entries.push(NameEntry {
            label: nm.to_string(),
            x_pau: cx as i32,
            y_pau: cy as i32,
            city: city.map(|(_, c)| c),
        });
    }
    (entries, city_of)
}

/// Nearest `place=` node (name + coords) — the street's city anchor — by squared PAU distance. Falls
/// back to `None` only when the input has no settlement at all (empty gazetteer).
fn nearest_place<'a>(
    places: &[(&'a String, (i64, i64))],
    x: i64,
    y: i64,
) -> Option<(&'a String, (i32, i32))> {
    let mut best: Option<(i128, &String, (i64, i64))> = None;
    for &(name, (px, py)) in places {
        let d2 = ((px - x) * (px - x) + (py - y) * (py - y)) as i128;
        if best.is_none_or(|(b, _, _)| d2 < b) {
            best = Some((d2, name, (px, py)));
        }
    }
    best.map(|(_, name, (px, py))| (name, (px as i32, py as i32)))
}

/// Write the street name-list and return the **device element ids** of its entries (name → element index).
/// The index must be read back from the *encoded* file — the element order is the trie's terminating-leaf
/// DFS order, not the insertion order (the GenAttr street-domain columns join through it, §11.6).
fn write_name_list_idx(
    path: &Path,
    entries: &[lid_format::NameEntry],
    region: u16,
    list_id: u16,
) -> (usize, HashMap<String, u32>) {
    let bytes = lid_format::encode_id(region, list_id, entries);
    if fs::write(path, &bytes).is_err() {
        eprintln!("write {path:?} failed");
        return (0, HashMap::new());
    }
    let mut idx = HashMap::new();
    if let Ok(nl) = lid_format::read(&bytes) {
        for (i, e) in nl.elements.iter().enumerate() {
            idx.entry(e.name.clone()).or_insert(i as u32);
        }
    }
    (entries.len(), idx)
}

/// Housenumber "12" → 12; "12A"/"3/5"/"31a"/"" → None (stock GenAttr is a *numeric* record column;
/// suffixed/compound numbers have no place in the `u32` number column — author tooling dropped them too).
fn hnr_number(s: &str) -> Option<u32> {
    let s = s.trim();
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        s.parse().ok()
    } else {
        None
    }
}

/// GenAttr house-number attribute block chunk size (street elements per TOC block; stock ≈4–19 k).
const HN_ATTR_CHUNK: usize = 8192;

/// Emit `LID40006.DAT` — the house-number GenAttr file (+20000, §11.6) in the **stock device
/// layout** (`SetDataBlock 00e09b60` / `enGetHnrIndices 00e0c3a0` / `enGetHnr 00e0d078`, §11.6b):
/// block element ranges tile the STREET name-list `0..nst`; `0xc01` (+0x28) = per-street house
/// numbers (existence/offsets/values), `0xc02` (+0x7c) per-record ref (street elem id), `0xc03..
/// 0xc06` empty per-record string lists (card requires them decodable), `0xc09/0xc0a/0xc0b/0xc0d`
/// per-record parity bits, `0xc11` (+0x380) per-record existence domain (the `enGetHnr` gate).
/// One record per numeric `addr:housenumber` joined to a street in `LID20006`.
fn write_gen_attr(
    path: &Path,
    d: &Data,
    bbox: Option<(f64, f64, f64, f64)>,
    street_idx: &HashMap<String, u32>,
    nst: usize,
    region: u16,
) -> usize {
    use lid_format::write::{write_gen_attr_file, BlockData, ColData, ColKind};
    use std::collections::BTreeMap;

    // street element id -> its house numbers (numeric only, street present in the name-list, in bbox).
    let mut by_street: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut nrec_all = 0usize;
    for (num, c, street) in &d.addrs {
        if !in_bbox(*c, bbox) {
            continue;
        }
        let Some(sid) = street_idx.get(street.trim()).copied() else {
            continue;
        };
        let Some(n) = hnr_number(num) else { continue };
        by_street.entry(sid).or_default().push(n);
        nrec_all += 1;
    }
    if nst < 2 || nrec_all == 0 {
        return 0;
    } // <2 blocks is invalid for the container; no addresses ⇒ nothing to write

    // Always >= 2 blocks tiling [0, nst).
    let nblk = (nst as usize).div_ceil(HN_ATTR_CHUNK).max(2);
    let w = (nst as usize).div_ceil(nblk);
    let mut blocks = Vec::new();
    for lo in (0..nst as usize).step_by(w) {
        let hi = (lo + w).min(nst as usize);
        let width = (hi - lo) as u32;
        let mut exists = vec![false; width as usize];
        let mut offs: Vec<u32> = Vec::new();
        let mut nums: Vec<u32> = Vec::new();
        let mut refs: Vec<u32> = Vec::new();
        let mut ev: Vec<bool> = Vec::new();
        let mut od: Vec<bool> = Vec::new();
        for (&sid, list) in by_street.range(lo as u32..hi as u32) {
            exists[(sid - lo as u32) as usize] = true;
            offs.push(nums.len() as u32);
            let mut list = list.clone();
            list.sort_unstable();
            for &n in &list {
                nums.push(n);
                refs.push(sid);
                ev.push(n % 2 == 0);
                od.push(n % 2 == 1);
            }
        }
        let nrec = nums.len();
        let dom1 = |n: u32| ColData {
            selector: 0xc11,
            kind: ColKind::ValueList,
            domain: n,
            exists: vec![true; n as usize],
            counts: (0..n).collect(),
            values: (0..n).collect(),
            bits: vec![],
            code_8000: 0x16,
            code_0000: 0x14,
            range_from_to: None,
        };
        let bits_col = |selector: u16, bits: Vec<bool>| ColData {
            selector,
            kind: ColKind::Binary,
            domain: nrec as u32,
            exists: vec![],
            counts: vec![],
            values: vec![],
            bits,
            code_8000: 0x16,
            code_0000: 0x14,
            range_from_to: None,
        };
        let es_col = |selector: u16| ColData {
            selector,
            kind: ColKind::EmptyByteList,
            domain: nrec as u32,
            exists: vec![],
            counts: vec![],
            values: vec![],
            bits: vec![],
            code_8000: 0x11,
            code_0000: 0x11,
            range_from_to: None,
        };
        let mut cols = vec![
            ColData {
                selector: 0xc01,
                kind: ColKind::ValueList,
                domain: width,
                exists,
                counts: offs,
                values: nums,
                bits: vec![],
                code_8000: 0x16,
                code_0000: 0x14,
                range_from_to: None,
            },
            ColData {
                selector: 0xc02,
                kind: ColKind::SingleValue,
                domain: nrec as u32,
                exists: vec![true; nrec],
                counts: vec![],
                values: refs,
                bits: vec![],
                code_8000: 0x16,
                code_0000: 0x14,
                range_from_to: None,
            },
            dom1(nrec as u32),
            bits_col(0xc09, ev),
            bits_col(0xc0a, od),
            bits_col(0xc0b, vec![false; nrec]),
            bits_col(0xc0d, vec![false; nrec]),
            es_col(0xc03),
            es_col(0xc04),
            es_col(0xc05),
            es_col(0xc06),
        ];
        cols.retain(|c| nrec > 0 || c.selector == 0xc01);
        blocks.push(BlockData {
            elem_start: lo as u32,
            elem_end: (hi - 1) as u32,
            cols,
        });
    }
    let outer = lid_format::header::nl_header(lid_format::header::KIND_GEN_ATTR, region, 3, 0);
    let bytes = write_gen_attr_file(nst as u32, &outer, &blocks);
    if fs::write(path, &bytes).is_err() {
        eprintln!("write {path:?} failed");
        return 0;
    }
    nrec_all
}

/// Point-address block width (street elements per DETAIL block; stock block sizes were ~k-10k).
const PA_CHUNK: usize = 4096;

/// Emit `PA_20006.DAT` — the point-address (fileType 0x18) sidecar of the STREET list: one access
/// point per street element. Device model (§11.7b, `bGetPACells 00be072c` / `00e0f8c4`): DETAIL
/// rows `0xd0b..0xd0f` = cell id / left / right / ratio / RELATIVE position; the device ADDS the
/// street element's own name-list anchor, so we store `house − street_centroid`. `cell` and `ratio`
/// existence bitmaps are all-set ([OPEN] NLCellID semantics unknown ⇒ neutral 0; ratio 100).
/// Position only for streets that have a numeric address — every other element keeps `None`, and
/// the device falls back to the anchor itself (= same result as shipping no PA file at all).
fn write_pa(
    path: &Path,
    d: &Data,
    bbox: Option<(f64, f64, f64, f64)>,
    entries: &[lid_format::NameEntry],
    street_idx: &HashMap<String, u32>,
    nst: usize,
    region: u16,
) -> usize {
    use lid_format::pa::{write_pa_file, PaDetailBlock, PaDetailEntry};
    use std::collections::BTreeMap;

    // street element -> lowest numeric address point (coords), if any.
    let mut best: BTreeMap<u32, (u32, (i64, i64))> = BTreeMap::new();
    for (num, c, street) in &d.addrs {
        if !in_bbox(*c, bbox) {
            continue;
        }
        let Some(sid) = street_idx.get(street.trim()).copied() else {
            continue;
        };
        let Some(n) = hnr_number(num) else { continue };
        match best.get(&sid) {
            Some(&(bn, _)) if bn <= n => {}
            _ => {
                best.insert(sid, (n, *c));
            }
        }
    }
    if nst == 0 || best.is_empty() {
        return 0;
    }

    let mut filled = vec![
        PaDetailEntry {
            cell: 0,
            left: false,
            right: false,
            ratio: 100,
            pos: None
        };
        nst
    ];
    let mut npa = 0usize;
    for e in entries {
        let Some(&sid) = street_idx.get(e.label.as_str()) else {
            continue;
        };
        if sid as usize >= nst {
            continue;
        }
        if let Some((_, (ax, ay))) = best.get(&sid) {
            // relative PAU offset from the street anchor (the device adds the anchor back).
            let dx = ax - i64::from(e.x_pau);
            let dy = ay - i64::from(e.y_pau);
            if i64::from(i32::MIN) <= dx && dx <= i64::from(i32::MAX)
                && i64::from(i32::MIN) <= dy && dy <= i64::from(i32::MAX)
            {
                filled[sid as usize].pos = Some((dx as i32, dy as i32));
                npa += 1;
            }
        }
    }
    let nblk = nst.div_ceil(PA_CHUNK);
    let w = nst.div_ceil(nblk);
    let blocks: Vec<PaDetailBlock> = (0..nblk)
        .map(|i| PaDetailBlock {
            entries: filled[i * w..((i + 1) * w).min(nst)].to_vec(),
        })
        .collect();
    // coord_mode [OPEN]: bDecodeLists consumes it as a parameter; 0 = PAU (name-list flavour).
    let bytes = write_pa_file(region, 3, nst as u32, 0, &blocks);
    if fs::write(path, &bytes).is_err() {
        eprintln!("write {path:?} failed");
        return 0;
    }
    npa
}

/// Emit `REL00001.DAT` — the street↔city relation matrix the stock META0000 relation table expects at
/// index 1 (`{from=listID 2 TOWN, to=listID 3 STREET}`), which the file stores as d0=3/d1=2 (rows =
/// street elements of `LID20006`, cols = city elements of `LID20001`; stock file 00001 = same shape).
/// One pair per street: its element ↔ the city element of its anchor place (§11.7).
fn write_street_city_rel(
    path: &Path,
    entries: &[lid_format::NameEntry],
    city_of: &[Option<String>],
    street_idx: &HashMap<String, u32>,
    city_idx: &HashMap<String, u32>,
) -> usize {
    let src_elems = street_idx.len() as u64;
    let tgt_elems = city_idx.len() as u64;
    let mut rels: Vec<(u32, u32)> = Vec::new();
    for (e, city) in entries.iter().zip(city_of) {
        let (Some(s), Some(cn)) = (street_idx.get(e.label.as_str()), city) else {
            continue;
        };
        if let Some(t) = city_idx.get(cn.as_str()) {
            rels.push((*s, *t));
        }
    }
    if src_elems == 0 || tgt_elems == 0 || rels.is_empty() {
        return 0;
    }
    match lid_format::rel::write_rel(src_elems, tgt_elems, 3, 2, &rels) {
        Ok(bytes) => {
            if fs::write(path, &bytes).is_err() {
                eprintln!("write {path:?} failed");
                return 0;
            }
            rels.len()
        }
        Err(e) => {
            eprintln!("REL00001: {e}");
            0
        }
    }
}

/// Emit the settlement name-list — the stock `LID20001.DAT` for **listID 2 (TOWN)** (the HNR-domain
/// `LID20000` is listID 129 — see the inventory table §11). One element per unique place name —
/// **without coordinates**: the stock settlement name-list carries none (empty `0x407` streams, origin
/// `-1/-1`, §12.5), so the writer keeps `city: None`. Returns `(count, name → device element id)`.
#[allow(clippy::type_complexity)]
fn write_cities(
    path: &Path,
    d: &Data,
    bbox: Option<(f64, f64, f64, f64)>,
    region: u16,
    list_id: u16,
) -> (usize, HashMap<String, u32>) {
    use lid_format::NameEntry;
    let mut seen: HashSet<String> = HashSet::new();
    let mut entries: Vec<NameEntry> = Vec::new();
    for (name, c, _kind) in &d.places {
        let nm = name.trim();
        if nm.is_empty() || nm.contains('\0') || !in_bbox(*c, bbox) {
            continue;
        }
        if !seen.insert(nm.to_string()) {
            continue;
        }
        entries.push(NameEntry {
            label: nm.to_string(),
            x_pau: c.0 as i32,
            y_pau: c.1 as i32,
            city: None,
        });
    }
    let bytes = lid_format::encode_id(region, list_id, &entries);
    if fs::write(path, &bytes).is_err() {
        eprintln!("write {path:?} failed");
        return (0, HashMap::new());
    }
    let mut idx = HashMap::new();
    if let Ok(nl) = lid_format::read(&bytes) {
        for (i, e) in nl.elements.iter().enumerate() {
            idx.entry(e.name.clone()).or_insert(i as u32);
        }
    }
    (entries.len(), idx)
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
    if p.len() == 4 {
        Some((p[0], p[1], p[2], p[3]))
    } else {
        None
    }
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
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | ' ') {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
