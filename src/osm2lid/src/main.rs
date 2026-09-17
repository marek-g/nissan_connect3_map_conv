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

use std::collections::{BTreeMap, HashMap, HashSet};
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
    // every routable-class highway, parse order, for onecell/cluster replication (osm2rnw rules)
    hw: Vec<(Vec<i64>, Option<String>)>,
    addrs: Vec<(String, (i64, i64), String)>,  // (housenumber, coord, street) — GenAttr source
}

impl Data {
    fn new() -> Data {
        Data {
            nodes: HashMap::new(),
            places: Vec::new(),
            pois: Vec::new(),
            streets: Vec::new(),
            hw: Vec::new(),
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
    let (naddr, street_cell) = if no_genattr {
        (0, Default::default())
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
    let npa = if no_pa {
        0
    } else {
        write_pa(
            &outdir.join("PA_20006.DAT"),
            &data,
            bbox,
            &st_entries,
            &street_idx,
            &street_cell,
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
                    let hw = tags.get("highway").is_some_and(|h| routable(h));
                    let addr =
                        tags.contains_key("addr:housenumber") && tags.contains_key("addr:street");
                    if street || addr || hw {
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
    if let Some(hw) = t.get("highway") {
        if routable(hw) {
            d.hw.push((ids.clone(), t.get("name").cloned()));
        }
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

/// Point-address block width (street elements per DETAIL block; stock block sizes were ~k-10k).
const PA_CHUNK: usize = 4096;

/// OSM `highway` values that osm2rnw treats as routable (classify() acceptance set — the only
/// tag that decides inclusion; junction/oneway affect attributes, not membership).
fn routable(hw: &str) -> bool {
    // THE set lives in rnw_model (osm2rnw blacklist ∧ classify == this whitelist).
    rnw_model::routable_way(hw)
}

/// One RNW onecell (2-node OSM way window), built exactly as `osm2rnw` builds its `segs` vector,
/// including the parse-order walk, the missing-node skip, the bbox check on the FIRST node only,
/// and the equal-node-id skip. `cluster` = the RNW cluster id the segment lands in: osm2rnw
/// numbers clusters by the order `build_clusters` pops them in its stack-DFS (write_nav passes
/// `index + 1`) — replicated here bit-for-bit so LID cell ids match a same-source RNW build.
#[derive(Clone)]
struct OneCell {
    ca: (i64, i64),
    cb: (i64, i64),
    name: Option<String>,
    cluster: u32,
}

fn build_onecells(d: &Data, bbox: Option<(f64, f64, f64, f64)>) -> Vec<OneCell> {
    let bb = bbox.map(|(w, s, e, n)| (deg2pau(w), deg2pau(s), deg2pau(e), deg2pau(n)));
    // THE segment walk + cluster split live in rnw_model, shared with osm2rnw — this is the
    // guarantee that 0x004 cluster ids match a same-source RNW build without reading it (§11.6b).
    let cells = rnw_model::walk_onecells(
        d.hw.iter().map(|(ids, name)| (ids.clone(), name.clone())),
        |id| d.nodes.get(&id).copied(),
        bb,
    );
    let groups = rnw_model::clusters_of(&cells, rnw_model::TARGET_ONECELLS);
    let ids = rnw_model::cluster_ids(&groups, cells.len());
    cells
        .into_iter()
        .zip(ids)
        .map(|(c, cluster)| OneCell {
            ca: c.ca,
            cb: c.cb,
            name: c.extra,
            cluster,
        })
        .collect()
}

fn pt_seg_d2(p: (i64, i64), a: (i64, i64), b: (i64, i64)) -> i128 {
    let (px0, py0) = (p.0 as i128, p.1 as i128);
    let (ax, ay) = (a.0 as i128, a.1 as i128);
    let (bx, by) = (b.0 as i128, b.1 as i128);
    let (vx, vy) = (bx - ax, by - ay);
    let len2 = (vx * vx + vy * vy).max(1);
    let t = ((px0 - ax) * vx + (py0 - ay) * vy).clamp(0, len2);
    let dx = px0 - (ax + vx * t / len2);
    let dy = py0 - (ay + vy * t / len2);
    dx * dx + dy * dy
}

/// One record per numeric `addr:housenumber` joined to a street in `LID20006`. Cell model as
/// reverse-engineered from the card stock (§11.6c `enGetHnrCellIndices 00e0c8bc` +
/// `enDecodeCells 00e0c584`): the block table row = one RNW **onecell segment** (0x004 = the RNW
/// cluster id a same-source `osm2rnw` build writes — verified equal segment/cluster counts on a
/// shared extract), records = one per (segment, parity) with `0xc01..0xc02` min..max and parity
/// bits `0xc09/0xc0a` (mirrored into `0xc0b/0xc0c`), `0xc11` = per-record row ordinal (both
/// parities of a segment cite the same row — the stock GRÓJECKA shape), and `0x001` = a unique
/// per-record author id. Addresses are assigned to the nearest segment of their street; streets
/// whose ways never passed the routable filter get one synthetic row (their own cluster id) so
/// their numbers are still shipped. [OPEN] stock hnr cluster ids are an author-side registry
/// numbering, not raw RNW ids — pairing our own osm2rnw + osm2lid output is self-consistent but
/// not card-verified.
fn write_gen_attr(
    path: &Path,
    d: &Data,
    bbox: Option<(f64, f64, f64, f64)>,
    street_idx: &HashMap<String, u32>,
    nst: usize,
    region: u16,
) -> (usize, BTreeMap<u32, u32>) {
    use lid_format::write::{write_gen_attr_file, BlockData, ColData, ColKind};
    use std::collections::{BTreeMap, BTreeSet};

    let segs = build_onecells(d, bbox);
    let ncl = segs.iter().map(|s| s.cluster).max().unwrap_or(0);
    eprintln!("onecells: {} clusters: {}", segs.len(), ncl);
    let cluster_next = ncl + 1;

    // street element id -> its onecells (global segment ordinals, in segment order)
    let mut segs_by_sid: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, sg) in segs.iter().enumerate() {
        if let Some(nm) = &sg.name {
            if let Some(&sid) = street_idx.get(nm.trim()) {
                segs_by_sid.entry(sid).or_default().push(i);
            }
        }
    }

    // Numeric addresses in bbox joined to the name list -> even/odd number buckets per segment
    // (key VIRT+sid = street without any routable segment, one synthetic row for the whole way).
    const VIRT: usize = usize::MAX / 2;
    let mut seg_even: HashMap<usize, Vec<u32>> = HashMap::new();
    let mut seg_odd: HashMap<usize, Vec<u32>> = HashMap::new();
    let mut virt_cluster: HashMap<u32, u32> = HashMap::new(); // streetless sid -> synthetic cluster
    let mut virt_cnt: HashMap<u32, (i128, i128, u32)> = HashMap::new();
    let mut owned: BTreeSet<u32> = BTreeSet::new(); // streets with at least one numeric address
    let mut nrec_all = 0usize;
    for (num, c, street) in &d.addrs {
        if !in_bbox(*c, bbox) {
            continue;
        }
        let Some(sid) = street_idx.get(street.trim()).copied() else {
            continue;
        };
        let Some(n) = hnr_number(num) else { continue };
        let key = match segs_by_sid.get(&sid) {
            Some(list) if !list.is_empty() => {
                let mut best = list[0];
                let mut bd = pt_seg_d2(*c, segs[best].ca, segs[best].cb);
                for &i in list.iter().skip(1) {
                    let dd = pt_seg_d2(*c, segs[i].ca, segs[i].cb);
                    if dd < bd {
                        bd = dd;
                        best = i;
                    }
                }
                best
            }
            _ => {
                let e = virt_cnt.entry(sid).or_insert((0, 0, 0));
                e.0 += i128::from(c.0);
                e.1 += i128::from(c.1);
                e.2 += 1;
                VIRT + sid as usize
            }
        };
        if n % 2 == 0 {
            seg_even.entry(key).or_default().push(n);
        } else {
            seg_odd.entry(key).or_default().push(n);
        }
        owned.insert(sid);
        nrec_all += 1;
    }
    if nst < 2 || nrec_all == 0 {
        return (0, Default::default());
    } // <2 blocks is invalid for the container; no addresses ⇒ nothing to write
    for &sid in virt_cnt.keys() {
        virt_cluster.insert(sid, cluster_next);
    }

    // Always >= 2 blocks tiling [0, nst).
    let nblk = (nst as usize).div_ceil(HN_ATTR_CHUNK).max(2);
    let w = (nst as usize).div_ceil(nblk);
    let mut blocks = Vec::new();
    // sid -> (lowest numeric record, its 0xc11 table row): the PA/destination cell for the street
    // (bGetPACellIDs 00b898dc feeds its cell ids straight into the street block's enGetCells).
    let mut street_cell: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
    let mut gid = 0u32; // author-space id for column 0x001: unique per record, ever-increasing
    for lo in (0..nst as usize).step_by(w) {
        let hi = (lo + w).min(nst as usize);
        let width = (hi - lo) as u32;
        let mut exists = vec![false; width as usize];
        let mut offs: Vec<u32> = Vec::new();
        let mut nums: Vec<u32> = Vec::new();
        let mut tos: Vec<u32> = Vec::new(); // 0xc02 per-record `to` bound (NLHnr+0xc)
        let mut ev: Vec<bool> = Vec::new();
        let mut od: Vec<bool> = Vec::new();
        let mut rec_row: Vec<u32> = Vec::new(); // 0xc11 per-record table row (both sides share one)
        let mut rec_id: Vec<u32> = Vec::new(); // 0x001 per-record author id
        let mut row_cluster: Vec<u32> = Vec::new(); // table row -> RNW cluster id (0x004)
        for &sid in owned.range(lo as u32..hi as u32) {
            // rows for this street: its segments that actually carry numbers, else the synthetic
            let mut street_rows: Vec<(usize, u32)> = Vec::new(); // (seg idx | VIRT+sid, cluster)
            if let Some(list) = segs_by_sid.get(&sid) {
                for &i in list {
                    if seg_even.contains_key(&i) || seg_odd.contains_key(&i) {
                        street_rows.push((i, segs[i].cluster));
                    }
                }
            }
            if street_rows.is_empty() && virt_cluster.contains_key(&sid) {
                street_rows.push((VIRT + sid as usize, virt_cluster[&sid]));
            }
            if street_rows.is_empty() {
                continue;
            }
            exists[(sid - lo as u32) as usize] = true;
            offs.push(nums.len() as u32);
            for &(seg, cl) in &street_rows {
                let base = row_cluster.len() as u32;
                row_cluster.push(cl);
                for (odd, bucket) in [(false, seg_even.get(&seg)), (true, seg_odd.get(&seg))] {
                    let Some(list) = bucket else { continue };
                    let mut list = list.clone();
                    list.sort_unstable();
                    let (mn, mx) = (list[0], *list.last().unwrap());
                    nums.push(mn);
                    tos.push(mx);
                    ev.push(!odd);
                    od.push(odd);
                    rec_row.push(base);
                    gid += 1;
                    rec_id.push(gid);
                    match street_cell.get(&sid) {
                        Some(&(mn0, _)) if mn0 <= mn => {}
                        _ => {
                            street_cell.insert(sid, (mn, base));
                        }
                    }
                }
            }
        }
        let nrec = nums.len();
        let nrow = row_cluster.len();
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
        let simple = |selector: u16, values: Vec<u32>, code: u16| ColData {
            selector,
            kind: ColKind::Simple32,
            domain: 0,
            exists: vec![],
            counts: vec![],
            values,
            bits: vec![],
            code_8000: 0x16,
            code_0000: code,
            range_from_to: None,
        };
        // 0xc11: per-record ROW ordinal(s) — the device ValueList allows 1..N values per record
        // (`enGetHnrCellIndices` 00e0c8bc); we follow the stock shape: one flat value per record,
        // both parities of a segment citing the same row.
        let c11 = ColData {
            selector: 0xc11,
            kind: ColKind::ValueList,
            domain: nrec as u32,
            exists: vec![true; nrec],
            counts: (0..nrec as u32).collect(),
            values: rec_row,
            bits: vec![],
            code_8000: 0x16,
            code_0000: 0x14,
            range_from_to: None,
        };
        // Block cell table (kolumny bound to NLCellIdAttrVector @+0x40/+0x530, §11.6c):
        // one row per segment; 0x002 = source NAV file no. (0 = single NAV file build),
        // 0x004 = RNW cluster id, 0x005 = per-row existence bits.
        let one_per_row: Vec<u32> = vec![1; nrow];
        let row_ord: Vec<u32> = (0..nrow as u32).collect();
        let mut cols = vec![
            ColData {
                selector: 0x0001,
                kind: ColKind::ValueList,
                domain: width,
                exists: exists.clone(),
                counts: offs.clone(),
                values: rec_id,
                bits: vec![],
                code_8000: 0x16,
                code_0000: 0x14,
                range_from_to: None,
            },
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
                values: tos,
                bits: vec![],
                code_8000: 0x16,
                code_0000: 0x14,
                range_from_to: None,
            },
            c11,
            bits_col(0xc09, ev),
            bits_col(0xc0a, od.clone()),
            bits_col(0xc0b, od.clone()), // parity mirror (stock: c0b==c09, c0c==c0a)
            bits_col(0xc0c, od),
            bits_col(0xc0d, vec![false; nrec]), // direction flag: [OPEN], stock writes ~half set
            es_col(0xc03),
            es_col(0xc04),
            es_col(0xc05),
            es_col(0xc06),
        ];
        cols.push(ColData {
            selector: 0x4003,
            kind: ColKind::Rows(vec![(0x4003, 0x03, nrow as u32, Vec::new())]),
            domain: nrow as u32,
            exists: vec![],
            counts: vec![],
            values: vec![],
            bits: vec![],
            code_8000: 0x16,
            code_0000: 0x14,
            range_from_to: None,
        });
        cols.push(ColData {
            selector: 0x0002,
            kind: ColKind::Simple16,
            domain: 0,
            exists: vec![],
            counts: vec![],
            values: vec![0; nrow],
            bits: vec![],
            code_8000: 0x16,
            code_0000: 0x11,
            range_from_to: None,
        });
        cols.push(simple(0x8003, one_per_row.clone(), 0x16));
        cols.push(simple(0x0003, row_ord.clone(), 0x14));
        cols.push(simple(0x8004, one_per_row, 0x16));
        cols.push(ColData {
            selector: 0x0004,
            kind: ColKind::Simple32,
            domain: 0,
            exists: vec![],
            counts: vec![],
            values: row_cluster,
            bits: vec![],
            code_8000: 0x16,
            code_0000: 0x18,
            range_from_to: None,
        });
        cols.push(ColData {
            selector: 0x0005,
            kind: ColKind::Binary,
            domain: nrow as u32,
            exists: vec![],
            counts: vec![],
            values: vec![],
            bits: vec![true; nrow],
            code_8000: 0x16,
            code_0000: 0x14,
            range_from_to: None,
        });
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
        return (0, Default::default());
    }
    (nrec_all, street_cell.into_iter().map(|(sid, (_, row))| (sid, row)).collect())
}

fn write_pa(
    path: &Path,
    d: &Data,
    bbox: Option<(f64, f64, f64, f64)>,
    entries: &[lid_format::NameEntry],
    street_idx: &HashMap<String, u32>,
    street_cell: &BTreeMap<u32, u32>,
    nst: usize,
    region: u16,
) -> usize {
    use lid_format::pa::{write_pa_file, PaDetailBlock, PaDetailEntry};

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
                // bGetPACellIDs 00b898dc hands this id straight to the street block's enGetCells:
                // it must be the 0xc11 table ordinal of the street's cell (bGetCellOfBlock match).
                if let Some(&row) = street_cell.get(&sid) {
                    filled[sid as usize].cell = row;
                }
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
