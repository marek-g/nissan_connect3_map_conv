// osm2lid — OSM (PBF or XML) -> Bosch TravelMap LID address/POI databases (SQLite).
//
// Generates, for one content region:
//   * GLOB_POI.DAT  — the POI gazetteer (SQLite FTS3, table GLOBAL_POIS). Cities + POIs.
//   * DB_CITY.DAT   — the global addressable-city list (SQLite FTS3, GlobalCityList).
// Both match the exact schemas the car reads (LID_format.md §3 / §10.5) and are validated
// by the reader `lid2dump` (round-trip). Coordinates are PAU = deg * 2^31 / 180.
//
// Scope note: writes the SQLite half (cities + POIs), the street + city name-lists
// (`LID20006.DAT` / `LID20000.DAT`, the ASF columnar trie, §12) built by `src/lid_format::encode`, the
// house-number GenAttr half (`LID40006.DAT`, §11.6) built by `src/lid_format::write` from OSM
// `addr:housenumber` objects, and the point-access (`PA_20006.DAT`, §11.7) half. Addresses are
// joined to streets through the resolver chain (`resolve_addresses`): exact name -> token/initial
// match -> near-named-segment fallback -> pseudo-street registration for `addr:place`/orphans
// (stock "DEBINY" model). Crossing (+10000) files are still pending. The street/city name-lists are
// validated by `lid2dump` (full OSM -> LID -> dump round-trip, position- and name-exact); the GenAttr
// writer is validated offline by `lid_format`'s `gen_attr_*` tests (byte-exact rebuild oracle + a
// device read-model round-trip) and end-to-end by `tests/{genattr,pa_cells,coverage}.rs`.
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
    addrs: Vec<(String, (i64, i64), String, bool)>, // (housenumber, coord, label, from_addr_place) — GenAttr source
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
    let mut no_crossings = false;
    let mut no_addr_list = false;
    let mut stock_city_map: Option<String> = None;
    let mut auto_city = false;
    let mut merge_stock: Option<String> = None;
    let mut city_radius: i64 = 500_000;
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
            "--no-crossings" => no_crossings = true,
            "--no-addr-list" => no_addr_list = true,
            "--auto-city" => auto_city = true,
            "--stock-city-map" => {
                i += 1;
                stock_city_map = args.get(i).cloned();
            }
            "--merge-stock" => {
                i += 1;
                merge_stock = args.get(i).cloned();
            }
            "--city-radius" => {
                i += 1;
                city_radius = args
                    .get(i)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(city_radius);
            }
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

    // City-stack mode: with --stock-city-map the STOCK LID20001/DB_CITY/GLOB_POI stay on the
    // card (device city index needs per-element language+position columns we do not emit —
    // LID_format.md §10.4 vPopulateCityIndices); we only ship streets/HNR/PA + REL whose city
    // ids are looked up in the stock name-list.
    let stock_cities = stock_city_map.is_some() || auto_city;
    if auto_city && merge_stock.is_none() {
        eprintln!("--auto-city requires --merge-stock (reads the stock LID20001)");
        exit(1);
    }
    let npoi = if stock_cities {
        0
    } else {
        write_glob_poi(
            &outdir.join("GLOB_POI.DAT"),
            &data,
            region_id,
            lang,
            bbox,
            !no_poi,
        )
    };
    let ncity = if stock_cities {
        0
    } else {
        write_db_city(&outdir.join("DB_CITY.DAT"), &data, region_id, bbox)
    };
    let (ncit, city_idx, city_entries) = write_cities(
        &outdir.join("LID20001.DAT"),
        &data,
        bbox,
        region_id,
        2,
        !stock_cities,
    );
    let city_ids: HashMap<String, Vec<u32>> = if auto_city {
        // Deterministic city mapping: fold(name) equality against the stock LID20001 element
        // names (author adds "NN NNN " postal prefixes to many entries - strip them). Towns the
        // stock list does not carry are APPENDED to the stock name-list as brand-new city
        // elements (no fuzzy matching anywhere).
        let dir = merge_stock.as_ref().unwrap();
        let p = Path::new(dir).join("LID20001.DAT");
        let stock = fs::read(&p).unwrap_or_else(|e| {
            eprintln!("--auto-city {p:?}: {e}");
            exit(1);
        });
        let nl = lid_format::read(&stock).unwrap_or_else(|e| {
            eprintln!("--auto-city stock LID20001: {e}");
            exit(1);
        });
        let mut m: HashMap<String, Vec<u32>> = HashMap::new();
        for (i, e) in nl.elements.iter().enumerate() {
            let key = fold(&strip_postal(&e.name));
            m.entry(key).or_default().push(i as u32);
        }
        let origin = nl.origin;
        let mut out: HashMap<String, Vec<u32>> = HashMap::new();
        let mut append: Vec<lid_format::NameEntry> = Vec::new();
        let mut added: Vec<String> = Vec::new();
        for name in city_idx.keys() {
            if let Some(ids) = m.get(&fold(name)).cloned() {
                out.insert(name.clone(), ids);
            } else {
                added.push(name.clone());
            }
        }
        // Region filter (deterministic): stock city entries carry a 2-digit Polish postal-code
        // region prefix in the name ("32 063 KRZESZOWICE"); a same-named village elsewhere in
        // the country has a different prefix (all "PIASKI": 21/23/63 vs local 32). Expected
        // region = mode of the prefixes among all matched entries; entries with no prefix are
        // kept (no information), entries from a foreign prefix are dropped and reported.
        fn postal_prefix(name: &str) -> Option<String> {
            let p = name.get(..2)?;
            if p.bytes().all(|c| c.is_ascii_digit()) && name.as_bytes().get(2) == Some(&b' ') {
                Some(p.to_string())
            } else {
                None
            }
        }
        // Anchor town = matched town whose element center sits closest to the extracted
        // data's bbox center; its stock entries' postal prefix defines the expected region.
        let mut cx = 0i64;
        let mut cy = 0i64;
        for e in &city_entries {
            cx += e.x_pau as i64;
            cy += e.y_pau as i64;
        }
        let n_ce = city_entries.len().max(1) as i64;
        cx /= n_ce;
        cy /= n_ce;
        let mut region: Option<String> = None;
        let mut best: u64 = u64::MAX;
        for (name, ids) in &out {
            let ent = &city_entries[*city_idx.get(name).unwrap() as usize];
            let d = ((ent.x_pau as i64 - cx) as u64) + ((ent.y_pau as i64 - cy) as u64);
            if d < best {
                if let Some(r) = ids
                    .iter()
                    .filter_map(|&i| postal_prefix(&nl.elements[i as usize].name))
                    .next()
                {
                    best = d;
                    region = Some(r);
                }
            }
        }
        let region = region.map(|p| (p, 1usize));
        if let Some((region, _)) = &region {
            // WARN-ONLY for now: dropping needs the geo validator (LID20001 coord units are a
            // per-block private encoding and same-named villages without a postal prefix
            // (30 plain "PIASKI") are indistinguishable by name alone).
            let mut foreign = 0usize;
            let mut plain = 0usize;
            for (_, ids) in out.iter() {
                for &i in ids.iter() {
                    match postal_prefix(&nl.elements[i as usize].name) {
                        None => plain += 1,
                        Some(p) if p != *region => foreign += 1,
                        Some(_) => {}
                    }
                }
            }
            eprintln!(
                "auto-city: anchor postal region \"{region}\": {foreign} ids prefixed from other \
                 regions, {plain} ids unprefixed (kept; enable geo drop after anchor validator)"
            );
        }
        eprintln!(
            "auto-city: {} towns matched stock ids, {} to append {added:?}",
            out.len(),
            added.len()
        );
        if !append.is_empty() {
            match lid_format::merge_name_list(&stock, &append) {
                Ok((merged, base2)) => {
                    let _ = fs::write(outdir.join("LID20001.DAT"), &merged);
                    for (j, name) in added.iter().enumerate() {
                        out.insert(name.clone(), vec![base2 + j as u32]);
                    }
                }
                Err(e) => eprintln!(
                    "NOTE --auto-city: cannot append city elements yet ({e}); unmatched towns \
                     fall back to the nearest mapped city (stock LID20001 stays on the card)"
                ),
            }
        }
        out
    } else {
        match &stock_city_map {
            Some(p) => {
                let mut m: HashMap<String, Vec<u32>> = HashMap::new();
                let txt = fs::read_to_string(p).unwrap_or_else(|e| {
                    eprintln!("--stock-city-map {p}: {e}");
                    exit(1);
                });
                for line in txt.lines() {
                    if let Some((n, ids)) = line.split_once('\t') {
                        let v: Vec<u32> = ids
                            .split(',')
                            .filter_map(|x| x.trim().parse().ok())
                            .collect();
                        if !v.is_empty() {
                            m.insert(n.trim().to_string(), v);
                        }
                    }
                }
                let miss: Vec<&String> = city_idx
                    .keys()
                    .filter(|k| !m.contains_key(k.as_str()))
                    .collect();
                eprintln!(
                "stock-city-map: {}/{} towns mapped to stock city ids; no map (city omitted from REL): {miss:?}",
                city_idx.len() - miss.len(),
                city_idx.len()
            );
                m
            }
            None => city_idx
                .iter()
                .map(|(k, v)| (k.clone(), vec![*v]))
                .collect(),
        }
    };
    let segs = build_onecells(&data, bbox);
    eprintln!(
        "onecells: {} clusters: {}",
        segs.len(),
        segs.iter().map(|x| x.cluster).max().unwrap_or(0)
    );
    let (mut st_entries, mut city_of) = collect_street_entries(&data, bbox);
    let real_labels: Vec<String> = st_entries.iter().map(|e| e.label.clone()).collect();
    let (addr_hits, pseudo_settlements) = resolve_addresses(&data, bbox, &segs, &real_labels);
    // Register village/pseudo streets exactly like the stock card: author label as its own
    // name-list entry per 1200 m settlement cluster, city = nearest place (stock: "DEBINY" x12).
    {
        // One entry per settlement cluster — same label AND city repeats are separate elements:
        // the ASF model stores duplicate names as PARALLEL 0x00 leaf edges (encoder-side FIX
        // `encode_duplicate_names_roundtrip`; CHECKED stock: 843k (name,city) duplicate groups,
        // up to 105 per group). Entry order is deterministic (cluster sort).
        let places: Vec<(&String, (i64, i64))> =
            data.places.iter().map(|(n, c, _)| (n, *c)).collect();
        for (label, (cx, cy)) in &pseudo_settlements {
            let (city_coord, city_name) = match nearest_place(&places, *cx, *cy) {
                Some((nm, c)) => (Some(c), Some(nm.clone())),
                None => (None, None),
            };
            city_of.push(city_name);
            st_entries.push(lid_format::NameEntry {
                label: label.clone(),
                x_pau: *cx as i32,
                y_pau: *cy as i32,
                city: city_coord,
                belonging: None,
            });
        }
    }
    // nearest-mapped-city fallback: streets whose nearest `place` node has no stock city id
    // (unmapped villages) would get belonging=NULL and vanish from every city list (card-proven).
    // Fall back to the nearest MAPPED city within `city_radius` PAU (~11.9M PAU per degree).
    if stock_cities {
        let mut mapped: Vec<(&String, (i64, i64))> = Vec::new();
        {
            let mut seen: std::collections::HashSet<&String> = std::collections::HashSet::new();
            for (n, c, _) in &data.places {
                if city_ids.contains_key(n.as_str()) && seen.insert(n) {
                    mapped.push((n, *c));
                }
            }
        }
        for (e, town) in st_entries.iter_mut().zip(city_of.iter_mut()) {
            if town
                .as_ref()
                .is_some_and(|t| city_ids.contains_key(t.as_str()))
            {
                continue;
            }
            let mut best: Option<(i64, &String, (i64, i64))> = None;
            for (n, c) in &mapped {
                let d2 = (c.0 - e.x_pau as i64).pow(2) + (c.1 - e.y_pau as i64).pow(2);
                if d2 <= city_radius * city_radius && best.is_none_or(|(b, _, _)| d2 < b) {
                    best = Some((d2, n, *c));
                }
            }
            if let Some((_, n, c)) = best {
                *town = Some(n.clone());
                e.city = Some((c.0 as i32, c.1 as i32));
            }
        }
    }
    // street -> owning city element id (column 0x40c): the device lists a city's streets through
    // this in-file column, same ids REL00001 uses as targets (ambiguous names: first stock id).
    for (e, town) in st_entries.iter_mut().zip(city_of.iter()) {
        e.belonging = town
            .as_ref()
            .and_then(|t| city_ids.get(t))
            .and_then(|v| v.first())
            .copied();
    }
    // Device typeahead is BYTE-EXACT over the stored name: `LISA_tclHnrTree::bGetList` converts the
    // typed string to unicode values and walks trie edges with exact-code `bFind`, then confirms with
    // `bDoesStringMatch` = plain `memcmp` (CHECKED on DAPIAPP 00ca5f48/00ca410c/00ca5eb4). Car keyboard
    // types UPPERCASE, so a mixed-case/diacritic-only name is un-findable (card-confirmed: our
    // "Polnej Róży" invisible to filter "POL" although the REL row carried it). Stock convention:
    // line 1 = ASCII-folded type-in key, TAB, then the display line (`decode_name` cuts identically).
    for e in &mut st_entries {
        e.label = format!("{}\t{}", fold(&e.label), e.label);
    }
    // Element/DFS order must equal encoded-label byte order (mapping invariant in
    // `write_name_list_idx`): sort by the new label, city groups keeping first-seen rank, moved
    // atomically with the parallel `city_of` array.
    {
        let mut rank: HashMap<Option<(i32, i32)>, u32> = HashMap::new();
        for e in st_entries.iter() {
            let r = rank.len() as u32;
            rank.entry(e.city).or_insert(r);
        }
        let mut pairs: Vec<(lid_format::NameEntry, Option<String>)> =
            st_entries.drain(..).zip(city_of.drain(..)).collect();
        pairs.sort_by(|a, b| {
            rank[&a.0.city]
                .cmp(&rank[&b.0.city])
                .then_with(|| a.0.label.cmp(&b.0.label))
        });
        (st_entries, city_of) = pairs.into_iter().unzip();
    }
    let (nst, mut streets) =
        write_name_list_idx(&outdir.join("LID20006.DAT"), &st_entries, region_id, 3);
    // city coordinate registry (first node per unique name — same order `write_cities` numbers elements)
    let city_coords: std::collections::BTreeMap<&String, (i64, i64)> = {
        let mut m = std::collections::BTreeMap::new();
        for (n, c, _) in &data.places {
            m.entry(n).or_insert(*c);
        }
        m
    };
    let nrel = write_street_city_rel(
        &outdir.join("REL00001.DAT"),
        &st_entries,
        &city_of,
        &streets.sid_of_entry,
        &city_ids,
        &city_coords,
    );
    // --merge-stock DIR (raw stock LID20006.DAT + REL00001.DAT): overwrite our standalone outputs
    // with the CARD-VALIDATED splice (2026-09-21): stock bytes kept verbatim, our streets appended
    // as extra blocks with ids base.., our REL rows merged into the stock matrix (rows for the
    // city ids we cover replaced, stock grid d4..d7 reused).
    let mut id_lo = 0u32; // GenAttr tiles [id_lo, id_lo+nst); merge mode rebases our sids by +base
    if let Some(dir) = &merge_stock {
        use lid_format::rel;
        let read_raw = |p: String, what: &str| -> Vec<u8> {
            let b = fs::read(&p).unwrap_or_else(|e| {
                eprintln!("merge-stock: read {p}: {e}");
                exit(1);
            });
            if b.len() < 12 || &b[4..10] != b"CPRNAV" {
                b
            } else {
                eprintln!(
                    "merge-stock: {what} is CPRNAV-compressed; decompress the stock copy first"
                );
                exit(1);
            }
        };
        let sl = read_raw(format!("{dir}/LID20006.DAT"), "LID20006.DAT");
        let (merged, base) = lid_format::merge_name_list(&sl, &st_entries).unwrap_or_else(|e| {
            eprintln!("merge-stock LID20006: {e}");
            exit(1);
        });
        fs::write(outdir.join("LID20006.DAT"), &merged).unwrap();
        let sr = read_raw(format!("{dir}/REL00001.DAT"), "REL00001.DAT");
        let ridx = rel::RelIndex::parse(&sr).unwrap_or_else(|e| {
            eprintln!("merge-stock REL00001: {e}");
            exit(1);
        });
        let mut pairs =
            rel::get_relations(&sr, &ridx, true, 0, ridx.d[2] + 1).expect("stock rel query");
        let ourf = fs::read(outdir.join("REL00001.DAT")).unwrap();
        let oidx = rel::RelIndex::parse(&ourf).expect("our rel reparse");
        let ours = rel::get_relations(&ourf, &oidx, true, 0, oidx.d[2] + 1).expect("our rel query");
        let our_t: std::collections::HashSet<u32> = ours.iter().map(|&(_, t)| t).collect();
        pairs.retain(|&(_, t)| !our_t.contains(&t));
        pairs.extend(ours.iter().map(|&(s, t)| (s + base, t)));
        pairs.sort_unstable();
        pairs.dedup();
        let mr = rel::write_rel_grid(
            u64::from(base) + nst as u64,
            u64::from(ridx.d[3]),
            ridx.d[0] as u16,
            ridx.d[1] as u16,
            u64::from(ridx.d[4]),
            u64::from(ridx.d[5]),
            u64::from(ridx.d[6]),
            u64::from(ridx.d[7]),
            &pairs,
        )
        .unwrap_or_else(|e| {
            eprintln!("merge-stock REL write: {e}");
            exit(1);
        });
        fs::write(outdir.join("REL00001.DAT"), &mr).unwrap();
        // rebase our list-3 element ids so the companion files reference the MERGED element numbers
        for sid in &mut streets.sid_of_entry {
            if *sid != u32::MAX {
                *sid += base;
            }
        }
        for ids in streets.by_label.values_mut() {
            for (sid, _) in ids.iter_mut() {
                *sid += base;
            }
        }
        id_lo = base;
        eprintln!(
            "merge-stock: base {base}, our elems {nst}, merged pairs {} (city rows replaced: {})",
            pairs.len(),
            our_t.len()
        );
        eprintln!(
            "merge-stock: ship LID20006.DAT + REL00001.DAT + LID40006.DAT (ids rebased to base {base}){}; PA/LID30006/LID20000 are not spliced (skip them, stock stays)",
            if outdir.join("LID20001.DAT").exists() { " + LID20001.DAT (appended city elements)" } else { "" }
        );
    }
    // GenAttr col 0x001 / 0xc11 owner data (card-reverse-engineered 2026-09-22: stock col 0x001 is
    // the per-street list of owning LID20001 city ids — the ONLY thing `bHasValidOwner 00ce5cf4`
    // accepts; streets without it never appear in a city's street list). Same city set the REL
    // matrix uses: the entry's reported city + every city within 3 km (stock DEBINY shape).
    let mut street_owners: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    if !city_ids.is_empty() {
        let mut oseen: HashSet<(u32, u32)> = HashSet::new();
        for ((e, city), &s) in st_entries
            .iter()
            .zip(&city_of)
            .zip(streets.sid_of_entry.iter())
        {
            if s == u32::MAX {
                continue;
            }
            let mut add = |s: u32, cn: &Option<String>, oseen: &mut HashSet<(u32, u32)>| {
                if let Some(ids) = cn.as_ref().and_then(|c| city_ids.get(c.as_str())) {
                    for &t in ids {
                        if oseen.insert((s, t)) {
                            street_owners.entry(s).or_default().push(t);
                        }
                    }
                }
            };
            add(s, city, &mut oseen);
            for (cn, cc) in &city_coords {
                if pt_seg_d2((e.x_pau as i64, e.y_pau as i64), *cc, *cc) <= m2d2(3000.0) {
                    add(s, &Some(cn.to_string()), &mut oseen);
                }
            }
        }
    }
    let (naddr, street_cell) = if no_genattr {
        (0, Default::default())
    } else {
        write_gen_attr(
            &outdir.join("LID40006.DAT"),
            &addr_hits,
            &segs,
            &streets,
            &street_owners,
            id_lo,
            nst,
            region_id,
        )
    };
    // splice our rebased GenAttr blocks onto the stock HNR file (keep stock house numbers intact)
    if let (Some(dir), false) = (merge_stock.as_deref(), no_genattr) {
        let p4 = outdir.join("LID40006.DAT");
        match (fs::read(Path::new(dir).join("LID40006.DAT")), fs::read(&p4)) {
            (Ok(stock4), Ok(our4)) => match lid_format::write::merge_gen_attr(&stock4, &our4) {
                Ok(merged) => {
                    eprintln!(
                        "merge-stock: LID40006 spliced {:+.1} MB -> {:.1} MB",
                        (merged.len() - stock4.len()) as f64 / 1e6,
                        merged.len() as f64 / 1e6
                    );
                    let _ = fs::write(&p4, &merged);
                }
                Err(e) => eprintln!("merge-stock: LID40006 splice FAILED: {e}"),
            },
            _ => eprintln!("merge-stock: no stock LID40006.DAT in {dir} (skip HNR splice)"),
        }
    }
    let npa = if no_pa || merge_stock.is_some() {
        0
    } else {
        write_pa(
            &outdir.join("PA_20006.DAT"),
            &addr_hits,
            &st_entries,
            &streets,
            &street_cell,
            nst,
            region_id,
        )
    };
    let ncross = if no_crossings || merge_stock.is_some() {
        0
    } else {
        write_crossings(
            &outdir.join("LID30006.DAT"),
            &segs,
            &streets,
            nst,
            region_id,
        )
    };
    let n129 = if no_addr_list {
        0
    } else {
        write_addr_list(
            &outdir.join("LID20000.DAT"),
            &st_entries,
            &city_of,
            &streets,
            &addr_hits,
            region_id,
        )
    };

    eprintln!(
        "wrote {}/GLOB_POI.DAT ({}), DB_CITY.DAT ({}), LID20001.DAT ({} cities), LID20000.DAT ({} addr rows), LID20006.DAT ({} streets), REL00001.DAT ({} street→city pairs), LID40006.DAT ({} house numbers), PA_20006.DAT ({} access points), LID30006.DAT ({} crossing rows), REGION_ID=0x{:03x} ({})",
        out, npoi, ncity, ncit, n129, nst, nrel, naddr, npa, ncross, region_id, region.to_uppercase()
    );
    eprintln!("NOTE: ship the stock META0000.DAT unchanged — its relation table entry #1 is (2↔3), which is what REL00001.DAT carries.");
    eprintln!(
        "NOTE: crossing cells (positions) are NOT yet written (column RE pending) — the device can list partners per street but may not locate them. PA file card-test pending (no stock PA sample exists on any card)."
    );
}

fn usage() {
    eprintln!(
        "Usage: osm2lid <in.osm.pbf|in.osm> -o OUTDIR [opts]\n\
        \t--region POL   content region code (sets REGION_ID = regionIdent)\n\
        \t--lang 22      LANG_IDX tag written on GLOB_POI rows (language index)\n\
        \t--bbox W,S,E,N keep only entries within this lon/lat box\n\
        \t--no-poi       skip amenity/shop/tourism POIs (cities only)\n\
        \t--no-genattr   skip the LID40006.DAT house-number (GenAttr +20000) file\n\
        \t--no-pa        skip the PA_20006.DAT point-access-point file\n\
        \t--no-crossings skip the LID30006.DAT crossing (fileID+10000) file\
        \t--no-addr-list skip the LID20000.DAT listID-129 address gazetteer\n\
        \t--merge-stock DIR  splice streets into raw stock DIR/LID20006.DAT + REL00001.DAT
\t--city-radius N    fallback city match radius in PAU (default 500000)
        \t--stock-city-map TSV  city mode B: do NOT write LID20001/DB_CITY/GLOB_POI (stock card \n\
        \t                files stay); REL00001 city ids come from TSV 'name<TAB>id[,id...]'\n\
        \t                (element ids of the stock LID20001; device city index needs the stock \n\
        \t                per-element language+position columns — see LID_format.md sec.10.4)"
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
    const REL: [&str; 9] = [
        "name",
        "place",
        "amenity",
        "shop",
        "tourism",
        "craft",
        "addr:housenumber",
        "addr:street",
        "addr:place",
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
                        let k: &str = &t.key; // addr:place fallback
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
        // addr:street (urban) and addr:place (village addressing) are kept apart: stock models
        // the place string as a pseudo-street name-list entry (DEBINY x12), never renames it.
        let (label, from_place) = match (t.get("addr:street"), t.get("addr:place")) {
            (Some(st), _) => (st.clone(), false),
            (None, Some(pl)) => (pl.clone(), true),
            _ => return,
        };
        d.addrs.push((num.clone(), c, label, from_place));
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
    let addr_street = t.get("addr:street").cloned();
    let addr_place = t.get("addr:place").cloned();
    if let (Some(num), Some((label, from_place))) = (
        t.get("addr:housenumber"),
        addr_street
            .map(|v| (v, false))
            .or_else(|| addr_place.map(|v| (v, true))),
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
                .push((num.clone(), (sx / k, sy / k), label, from_place));
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
            belonging: None,
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
/// Street labels ⇄ DECODED element ids. `encode_id` regroups entries by city (first-seen) and
/// the block trie reorders leaves by DFS, so decoded element order generally differs from input
/// order — and duplicate labels (stock-style pseudo-streets on parallel 0x00 leaf edges, up to
/// 105 per (name,city) group on the POL card) make a single label→id map unsound: numbers, PA
/// and REL must bind to the right DUPLICATE element. Alignment: within one label, the encoder
/// emits occurrences in (city first-seen rank, input order); decode ids ascend in exactly that
/// order (blocks follow the chunk sequence, parallel edges keep insertion order) — zip the two.
struct SidMap {
    /// label -> (decoded id, absolute entry pos) ascending by decoded id
    by_label: BTreeMap<String, Vec<(u32, (i64, i64))>>,
    /// input entry index -> decoded element id
    sid_of_entry: Vec<u32>,
}

impl SidMap {
    fn pick(&self, label: &str, c: (i64, i64)) -> Option<u32> {
        self.by_label.get(label).and_then(|v| {
            v.iter()
                .min_by_key(|(_, p)| pt_seg_d2(c, *p, *p))
                .map(|(i, _)| *i)
        })
    }
    fn first(&self, label: &str) -> Option<u32> {
        self.by_label
            .get(label)
            .and_then(|v| v.first())
            .map(|(i, _)| *i)
    }
}

/// Display part of a stored label: a label may carry the stock two-line form "KEY\tDISPLAY";
/// the device (and `lid_format::read`) take everything after the first TAB as the name.
fn disp(s: &str) -> &str {
    s.split_once('\t').map_or(s, |(_, d)| d)
}

fn write_name_list_idx(
    path: &Path,
    entries: &[lid_format::NameEntry],
    region: u16,
    list_id: u16,
) -> (usize, SidMap) {
    let bytes = lid_format::encode_id(region, list_id, entries);
    if fs::write(path, &bytes).is_err() {
        eprintln!("write {path:?} failed");
        return (
            0,
            SidMap {
                by_label: BTreeMap::new(),
                sid_of_entry: vec![],
            },
        );
    }
    let mut decode_ids: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
    let nl = lid_format::read(&bytes).ok();
    if let Some(nl) = &nl {
        for (i, e) in nl.elements.iter().enumerate() {
            decode_ids
                .entry(e.name.as_str())
                .or_default()
                .push(i as u32);
        }
    }
    // city first-seen ranks (mirrors encode_id's grouping)
    let mut city_rank: HashMap<Option<(i32, i32)>, u32> = HashMap::new();
    for e in entries {
        let r = city_rank.len() as u32;
        city_rank.entry(e.city).or_insert(r);
    }
    let mut occ: BTreeMap<&str, Vec<(u32, usize, usize)>> = BTreeMap::new(); // display -> (rank, seq, input idx)
    let mut seq_cnt: HashMap<(&str, Option<(i32, i32)>), usize> = HashMap::new();
    for (i, e) in entries.iter().enumerate() {
        let rank = city_rank[&e.city];
        let s = seq_cnt.entry((e.label.as_str(), e.city)).or_default();
        occ.entry(disp(e.label.as_str()))
            .or_default()
            .push((rank, *s, i));
        *s += 1;
    }
    let mut by_label: BTreeMap<String, Vec<(u32, (i64, i64))>> = BTreeMap::new();
    let mut sid_of_entry = vec![u32::MAX; entries.len()];
    let mut degraded = false;
    for (label, mut o) in occ {
        let ids = decode_ids.get(label).cloned().unwrap_or_default();
        o.sort_unstable();
        if o.len() == ids.len() {
            for (&id, &(_, _, ei)) in ids.iter().zip(&o) {
                sid_of_entry[ei] = id;
                by_label.entry(label.to_string()).or_default().push((
                    id,
                    (i64::from(entries[ei].x_pau), i64::from(entries[ei].y_pau)),
                ));
            }
        } else {
            degraded = true; // decode disagrees with grouping model: fall back to first-wins
        }
        if let Some(v) = by_label.get_mut(label) {
            v.sort_unstable();
        }
    }
    if degraded {
        eprintln!("WARNING: name-list decode alignment mismatch, duplicate labels may mis-bind");
        for (i, e) in entries.iter().enumerate() {
            if sid_of_entry[i] == u32::MAX {
                if let Some(f) = decode_ids
                    .get(disp(e.label.as_str()))
                    .and_then(|v| v.first().copied())
                {
                    sid_of_entry[i] = f;
                }
            }
        }
    }
    (
        entries.len(),
        SidMap {
            by_label,
            sid_of_entry,
        },
    )
}

/// Housenumber "12" → 12; "12A"/"3/5"/"31a"/"" → None (stock GenAttr is a *numeric* record column;
/// suffixed/compound numbers have no place in the `u32` number column — author tooling dropped them too).
/// Parsed `addr:housenumber` (GenAttr cols `0xc01`/`0xc02` + parity mask `0xc09/0xc0a`).
/// Stock has no way to encode letters — both columns are u32 (CHECKED on POL stock: DEBINY rows
/// and city rows alike) — so alpha suffixes are cut away (`1a`/`11b` -> `1`/`11`, single parity
/// taken from the number; identical cut-offs on a segment dedup to one record, they are the same
/// building). Dash ranges (`12-16`, also en/em dash) and slash unions (`1/2`, `3/7`, `1A/2`)
/// become a `from..to` range with BOTH parity flags — the mass stock pattern for ranges
/// (hn_even & hn_odd both set, 467k rows in the POL card).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct HN {
    from: u32,
    to: u32,
    even: bool,
    odd: bool,
}

fn parse_hn(s: &str) -> Option<HN> {
    let mut parts: Vec<&str> = Vec::new();
    for piece in s.split(['-', '\u{2013}', '\u{2014}', '/']) {
        let digits: &str = {
            let t = piece.trim();
            let end = t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len());
            &t[..end]
        };
        if !digits.is_empty() {
            parts.push(digits);
        }
    }
    let num = |p: &&str| p.parse::<u32>().ok();
    match parts.len() {
        0 => None,
        1 => {
            let n = num(&parts[0])?;
            Some(HN {
                from: n,
                to: n,
                even: n % 2 == 0,
                odd: n % 2 == 1,
            })
        }
        _ => {
            let (from, to) = (
                parts
                    .iter()
                    .map(|p| num(p))
                    .collect::<Option<Vec<u32>>>()?
                    .into_iter()
                    .min()?,
                parts
                    .iter()
                    .map(|p| num(p))
                    .collect::<Option<Vec<u32>>>()?
                    .into_iter()
                    .max()?,
            );
            Some(HN {
                from,
                to,
                even: true,
                odd: true,
            })
        }
    }
}

#[cfg(test)]
mod hn_tests {
    use super::parse_hn;

    #[test]
    fn hn_parsing() {
        use super::HN;
        let cases: &[(&str, Option<HN>)] = &[
            (
                "7",
                Some(HN {
                    from: 7,
                    to: 7,
                    even: false,
                    odd: true,
                }),
            ),
            (
                "8 ",
                Some(HN {
                    from: 8,
                    to: 8,
                    even: true,
                    odd: false,
                }),
            ),
            (
                "1a",
                Some(HN {
                    from: 1,
                    to: 1,
                    even: false,
                    odd: true,
                }),
            ),
            (
                "1A",
                Some(HN {
                    from: 1,
                    to: 1,
                    even: false,
                    odd: true,
                }),
            ),
            (
                "11b",
                Some(HN {
                    from: 11,
                    to: 11,
                    even: false,
                    odd: true,
                }),
            ),
            (
                "31 A",
                Some(HN {
                    from: 31,
                    to: 31,
                    even: false,
                    odd: true,
                }),
            ),
            (
                "12-16",
                Some(HN {
                    from: 12,
                    to: 16,
                    even: true,
                    odd: true,
                }),
            ),
            (
                "12 \u{2013} 16",
                Some(HN {
                    from: 12,
                    to: 16,
                    even: true,
                    odd: true,
                }),
            ),
            (
                "1/2",
                Some(HN {
                    from: 1,
                    to: 2,
                    even: true,
                    odd: true,
                }),
            ),
            (
                "3/7",
                Some(HN {
                    from: 3,
                    to: 7,
                    even: true,
                    odd: true,
                }),
            ),
            (
                "2/1",
                Some(HN {
                    from: 1,
                    to: 2,
                    even: true,
                    odd: true,
                }),
            ),
            (
                "1A/2",
                Some(HN {
                    from: 1,
                    to: 2,
                    even: true,
                    odd: true,
                }),
            ),
            (
                "5-",
                Some(HN {
                    from: 5,
                    to: 5,
                    even: false,
                    odd: true,
                }),
            ),
            ("", None),
            ("b5", None),
            ("b", None),
            ("?", None),
        ];
        for (src, want) in cases {
            assert_eq!(parse_hn(src), *want, "parse_hn({src:?})");
        }
    }
}

/// Fold a lowercased Polish character to its diacritic-free base (folding = collapsing the
/// variants, not ignoring case): a`\u{0105}`->a, c`\u{0107}`->c, e`\u{0119}`->e, `\u{0142}`->l,
/// n`\u{0144}`->n, o`\u{00f3}`->o, s`\u{015b}`->s, z`\u{017a}`/z`\u{017c}`->z. OSM `addr:street`
/// is free text and is very often typed without Polish accents while the road `name` carries
/// them — folding both sides lets "Kosciuszki" find "Kościuszki". Risk (two real streets
/// differing ONLY by diacritics in one town) is covered by the nearest-candidate tie-break.
fn fold_diacritics(t: &str) -> String {
    t.chars()
        .map(|c| match c {
            '\u{0105}' => 'a',
            '\u{0107}' => 'c',
            '\u{0119}' => 'e',
            '\u{0142}' => 'l',
            '\u{0144}' => 'n',
            '\u{00f3}' => 'o',
            '\u{015b}' => 's',
            '\u{017a}' | '\u{017c}' => 'z',
            _ => c,
        })
        .collect()
}

/// Normalize a street/place label into comparable tokens: lowercased alphanumeric runs with
/// diacritics folded, Polish address-type prefixes dropped (`ul.`, `al.`, `os.` ...). Initials
/// survive as one-letter tokens (`K. Wyki` -> [k, wyki]).
fn street_tokens(label: &str) -> Vec<String> {
    label
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .map(fold_diacritics)
        .filter(|t| {
            !t.is_empty() && !matches!(t.as_str(), "ul" | "al" | "aleja" | "alei" | "os" | "ulica")
        })
        .collect()
}

/// `addr` tokens must appear as an ORDERED subsequence of the street's tokens; a one-letter
/// addr token (OSM initial style) matches any street token starting with the same letter
/// (diacritic-exact). "K. Wyki" <= "Kazimierza Wyki"; "Kazimierza Pułaskiego" <=
/// "Generała Kazimierza Pułaskiego". Rejects unrelated strings.
fn tokens_subsequence(addr: &[String], street: &[String]) -> bool {
    if addr.is_empty() || street.is_empty() {
        return false;
    }
    let mut it = street.iter();
    'next: for a in addr {
        for b in it.by_ref() {
            let ok = if a.chars().count() == 1 {
                b.chars().next() == a.chars().next()
            } else {
                a == b
            };
            if ok {
                continue 'next;
            }
        }
        return false;
    }
    true
}

#[cfg(test)]
fn _tokens_subseq_test(a: &str, b: &str) -> bool {
    tokens_subsequence(&street_tokens(a), &street_tokens(b))
}

#[cfg(test)]
mod match_tests {
    use super::_tokens_subseq_test as t;

    #[test]
    fn street_token_matching() {
        assert!(t("K. Wyki", "Kazimierza Wyki"));
        assert!(t("T. Kościuszki", "Tadeusza Kościuszki"));
        assert!(t("Kazimierza Pułaskiego", "Generała Kazimierza Pułaskiego"));
        assert!(t("ul. Piłsudskiego", "Pi\u{0142}sudskiego"));
        assert!(t("T. Kosciuszki", "Tadeusza Ko\u{015b}ciuszki")); // diacritic-free addr side
        assert!(t("slask", "\u{015a}l\u{0105}sk")); // folding is symmetric
        assert!(!t("W. Wladystawy", "\u{0141}adysława")); // w != ł-folded-l: correct reject
        assert!(t("W. Lokietka", "W\u{0142}adysława \u{0141}okietka")); // folded real initial matches
        assert!(!t("Zielona", "Kazimierza Wyki"));
        assert!(!t("Wyki K.", "Kazimierza Wyki")); // order matters (out-of-order is rarer; be strict)
        assert!(t("Wyki", "Kazimierza Wyki"));
    }
}

/// One address that passed number parsing and street targeting (before final element-index
/// binding, which needs the encoded name-list order).
struct AddrHit {
    /// Target street label — real register name (after the exact/token/geometry chain) or,
    /// for pseudo-streets, the author string itself (registered as a DEBINY-style name
    /// entry before `write_name_list_idx`, so label lookup binds it like any street).
    label: String,
    coord: (i64, i64),
    hn: HN,
    /// Raw `addr:housenumber` text as authored (kept for the listID-129 gazetteer row
    /// `"CITY, STREET NUMBER"`; `hn` is its GenAttr numeric projection).
    raw: String,
    /// Segment chosen among the target street's onecells; None => target has no own onecells
    /// (registered-but-unroutable street, or village pseudo-street) -> synthetic table row.
    seg: Option<usize>,
}

/// PAU-rectangular distance thresholds used by the resolver (1 m ~ 107.3 PAU).
const PAU_PER_M: f64 = PAU / 111_320.0;
fn m2d2(m: f64) -> i128 {
    let p = (m * PAU_PER_M) as i128;
    p * p
}

/// Bucket HNs into emitted records: single numbers of equal parity merge when consecutive
/// at step 2 (11,13,15 -> one odd record 11..15 — the stock `0xc01..0xc02` + single-parity
/// shape, cf. POL stock row 5..9 odd); gaps split runs; parsed multi-number ranges
/// (slash/dash, both parities) are kept verbatim. Sorted by (from, to), deduped.
fn compose_records(src: &[HN]) -> Vec<HN> {
    let mut out: Vec<HN> = Vec::new();
    let mut ev: Vec<u32> = Vec::new();
    let mut od: Vec<u32> = Vec::new();
    for h in src {
        if h.from == h.to && !(h.even && h.odd) {
            if h.odd {
                od.push(h.from);
            } else {
                ev.push(h.from);
            }
        } else {
            out.push(*h);
        }
    }
    for (mut list, even, odd) in [(ev, true, false), (od, false, true)] {
        list.sort_unstable();
        list.dedup();
        let mut i = 0usize;
        while i < list.len() {
            let from = list[i];
            let mut last = from;
            let mut j = i + 1;
            while j < list.len() && list[j] == last + 2 {
                last = list[j];
                j += 1;
            }
            out.push(HN {
                from,
                to: last,
                even,
                odd,
            });
            i = j;
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Emit the stock `LID20000.DAT` — the listID-129 address gazetteer (LID_format §11.2, stock POL
/// oracle: 194 118 rows, globally sorted, all names unique, no positions, blocks ~11.5k). Row
/// grammar: a plain `CITY` row per city that has any row, a `CITY, STREET` row per (city, street)
/// element pair (stock keeps these for streets whose addresses are numberless AND for the DEBINY
/// pseudo-settlements), and a `CITY, STREET NUMBER` row per resolved house number. The street
/// component is byte-identical to the LID20006 element name: the device re-derives a city's
/// street-index set from these very names (`LISA_tclHnrProcessing::bSetUpStreetIndcesByHnr` +
/// the `NLGenAttrHnrStreetIdxDetermination` descriptor), so any renormalisation here would break
/// that join. Encoder stores city-less entries in the stock "no coordinates" flavor (§12.5).
fn write_addr_list(
    path: &Path,
    entries: &[lid_format::NameEntry],
    city_of: &[Option<String>],
    streets: &SidMap,
    hits: &[AddrHit],
    region: u16,
) -> usize {
    use std::collections::BTreeSet;
    let mut sid_to_ei: HashMap<u32, usize> = HashMap::new();
    for (ei, &sid) in streets.sid_of_entry.iter().enumerate() {
        sid_to_ei.entry(sid).or_insert(ei);
    }
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut cities: BTreeSet<&String> = BTreeSet::new();
    for (e, city) in entries.iter().zip(city_of) {
        let Some(city) = city else { continue };
        cities.insert(city);
        // join keys are display strings (AddrHit labels carry the OSM display name, no type-in line)
        let d = e
            .label
            .split_once('\t')
            .map_or(e.label.as_str(), |(_, d)| d);
        names.insert(format!("{city}, {d}"));
    }
    for hit in hits {
        let Some(sid) = streets.pick(&hit.label, hit.coord) else {
            continue;
        };
        let Some(&ei) = sid_to_ei.get(&sid) else {
            continue;
        };
        let Some(Some(city)) = city_of.get(ei) else {
            continue;
        };
        names.insert(format!("{city}, {} {}", hit.label, hit.raw));
    }
    names.extend(cities.into_iter().cloned());
    if names.is_empty() {
        return 0; // stock has no empty list file; skip it entirely
    }
    let list: Vec<lid_format::NameEntry> = names
        .iter()
        .map(|n| lid_format::NameEntry {
            label: n.clone(),
            x_pau: -1,
            y_pau: -1,
            city: None,
            belonging: None,
        })
        .collect();
    let bytes = lid_format::encode_id(region, 129, &list);
    if fs::write(path, &bytes).is_err() {
        eprintln!("write {path:?} failed");
        return 0;
    }
    list.len()
}

/// Resolve every valid address to a street LABEL + (optionally) a segment, replicating what the
/// stock card encodes structurally (see module notes / LID_format §11.6b):
///  * `addr:street`:    1. label EXACTLY a register name -> that street;
///                      2. token subsequence/initial match against register names
///                      ("K. Wyki" -> "Kazimierza Wyki"; ties -> nearest candidate);
///                      3. else nearest NAMED routable segment within 120 m (median lateral
///                         offset of house points is 17 m, p90 83 m — 120 m is the measured
///                         safety net for initial/abbreviated tags);
///                      4. else pseudo-street (author string registered as its own name entry).
///  * `addr:place`:     pseudo-street straight away — stock models village addressing as
///                      name-list entries under the VILLAGE label split into spatial clusters,
///                      one entry per settlement ("DEBINY" x12 on the POL card), NOT geometry
///                      renaming (a village house sits at a through-road's segment and stock
///                      deliberately does not call it by that road's name).
/// Returns the hits (parse order, deterministic) plus the pseudo-street settlements to register:
/// (label, centroid), each cluster = union-find group of the label's addresses at 1200 m.
fn resolve_addresses(
    d: &Data,
    bbox: Option<(f64, f64, f64, f64)>,
    segs: &[OneCell],
    real_labels: &[String],
) -> (Vec<AddrHit>, Vec<(String, (i64, i64))>) {
    use std::collections::BTreeMap;
    let real: std::collections::HashSet<&str> = real_labels.iter().map(|x| x.as_str()).collect();
    // label -> its street's global segment ordinals (in segment order, ties -> first)
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, sg) in segs.iter().enumerate() {
        if let Some(nm) = sg.name.as_deref().map(str::trim) {
            if !nm.is_empty() {
                by_name.entry(nm).or_default().push(i);
            }
        }
    }
    let tok_of: BTreeMap<&str, Vec<String>> = real_labels
        .iter()
        .map(|l| (l.as_str(), street_tokens(l)))
        .collect();

    fn nearest_seg(c: (i64, i64), list: &[usize], segs: &[OneCell]) -> (usize, i128) {
        let mut best = list[0];
        let mut bd = pt_seg_d2(c, segs[best].ca, segs[best].cb);
        for &i in list.iter().skip(1) {
            let dd = pt_seg_d2(c, segs[i].ca, segs[i].cb);
            if dd < bd {
                bd = dd;
                best = i;
            }
        }
        (best, bd)
    }

    // label of chain step 2/3 for a failing string (None = fall through to pseudo)
    let mut fallback_cache: BTreeMap<String, Option<(String, Option<usize>)>> = BTreeMap::new();

    let mut hits: Vec<AddrHit> = Vec::new();
    let mut pseudo_groups: BTreeMap<String, Vec<(i64, i64)>> = BTreeMap::new();
    for (num, c, label, from_place) in &d.addrs {
        if !in_bbox(*c, bbox) {
            continue;
        }
        let Some(parsed) = parse_hn(num) else {
            continue;
        };
        let label = label.trim().to_string();
        if label.is_empty() || label.contains('\0') {
            continue;
        }
        // stock pattern for addr:place: the village label is kept as-is and clustered — never
        // geometry-renamed (a village house sits at a through-road segment and stock does NOT
        // call it by that road's name). A place string that literally IS a registered street
        // name binds to that street (same entry, no duplicate).
        if *from_place && !real.contains(label.as_str()) {
            pseudo_groups.entry(label.clone()).or_default().push(*c);
            hits.push(AddrHit {
                label,
                coord: *c,
                hn: parsed,
                raw: num.trim().to_string(),
                seg: None,
            });
            continue;
        }
        if real.contains(label.as_str()) {
            let seg = by_name
                .get(label.as_str())
                .map(|list| nearest_seg(*c, list, segs).0);
            hits.push(AddrHit {
                label,
                coord: *c,
                hn: parsed,
                raw: num.trim().to_string(),
                seg,
            });
            continue;
        }
        let target = fallback_cache.entry(label.clone()).or_insert_with(|| {
            let atok = street_tokens(&label);
            let mut cands: Vec<(&str, usize, i128)> = Vec::new();
            for name in real_labels {
                if tokens_subsequence(&atok, tok_of.get(name.as_str()).unwrap_or(&Vec::new())) {
                    if let Some(list) = by_name.get(name.as_str()) {
                        let (si, dd) = nearest_seg(*c, list, segs);
                        cands.push((name.as_str(), si, dd));
                    }
                }
            }
            if cands.len() == 1 {
                let (n, si, _) = cands[0];
                return Some((n.to_string(), Some(si)));
            }
            if cands.len() > 1 {
                let (n, si, _) = *cands.iter().min_by_key(|(_, _, d)| *d)?;
                return Some((n.to_string(), Some(si)));
            }
            // geometry fallback: nearest named routable segment within 120 m
            let mut best: Option<(i128, usize)> = None;
            for (i, sg) in segs.iter().enumerate() {
                if sg.name.as_deref().map(str::trim).unwrap_or("").is_empty() {
                    continue;
                }
                let dd = pt_seg_d2(*c, sg.ca, sg.cb);
                if dd <= m2d2(120.0) && best.is_none_or(|(bd, _)| dd < bd) {
                    best = Some((dd, i));
                }
            }
            best.map(|(_, i)| {
                let nm = segs[i].name.clone().unwrap_or_default().trim().to_string();
                (nm, Some(i))
            })
        });
        match target {
            Some((nm, seg)) => hits.push(AddrHit {
                label: nm.clone(),
                coord: *c,
                hn: parsed,
                raw: num.trim().to_string(),
                seg: *seg,
            }),
            None => {
                pseudo_groups.entry(label.clone()).or_default().push(*c);
                hits.push(AddrHit {
                    label,
                    coord: *c,
                    hn: parsed,
                    raw: num.trim().to_string(),
                    seg: None,
                });
            }
        }
    }

    // pseudo settlements: 1200 m union-find clusters per label, centroids, deterministic order
    let mut pseudo: Vec<(String, (i64, i64))> = Vec::new();
    for (label, pts) in pseudo_groups {
        let mut pts = pts;
        pts.sort_unstable();
        let mut parent: Vec<usize> = (0..pts.len()).collect();
        fn find(p: &mut Vec<usize>, mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for i in 0..pts.len() {
            for j in i + 1..pts.len() {
                if pt_seg_d2(pts[i], pts[j], pts[j]) <= m2d2(1200.0) {
                    let (a, b) = (find(&mut parent, i), find(&mut parent, j));
                    if a != b {
                        parent[a.max(b)] = a.min(b);
                    }
                }
            }
        }
        let mut clusters: BTreeMap<usize, (i128, i128, u32)> = BTreeMap::new();
        for (i, p) in pts.iter().enumerate() {
            let root = find(&mut parent, i);
            let e = clusters.entry(root).or_insert((0, 0, 0));
            e.0 += p.0 as i128;
            e.1 += p.1 as i128;
            e.2 += 1;
        }
        for (_, (sx, sy, n)) in clusters {
            pseudo.push((
                label.clone(),
                (
                    sx.div_euclid(n as i128) as i64,
                    sy.div_euclid(n as i128) as i64,
                ),
            ));
        }
    }
    pseudo.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    (hits, pseudo)
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
/// bits `0xc09/0xc0a` (mirrored into `0xc0b/0xc0c`), `0x001` = per-street owning-city id list and
/// `0xc11` = per-record owning STREET element id (stock card RE: `bHasValidOwner 00ce5cf4` drops every record
/// whose owner is neither 0xffffffff nor an id in the selected-city context — streets without
/// these columns are invisible in the device's city→street list). Addresses
/// are assigned to the nearest segment of their street; streets
/// whose ways never passed the routable filter get one synthetic row (their own cluster id) so
/// their numbers are still shipped. [OPEN] stock hnr cluster ids are an author-side registry
/// numbering, not raw RNW ids — pairing our own osm2rnw + osm2lid output is self-consistent but
/// not card-verified.
fn write_gen_attr(
    path: &Path,
    hits: &[AddrHit],
    segs: &[OneCell],
    streets: &SidMap,
    street_owners: &BTreeMap<u32, Vec<u32>>,
    id_lo: u32,
    n: usize,
    region: u16,
) -> (usize, BTreeMap<u32, u32>) {
    use lid_format::write::{write_gen_attr_file, BlockData, ColData, ColKind};
    use std::collections::{BTreeMap, BTreeSet};

    let ncl = segs.iter().map(|s| s.cluster).max().unwrap_or(0);
    let cluster_next = ncl + 1;

    // street element id -> its onecells (global segment ordinals, in segment order)
    let mut segs_by_sid: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, sg) in segs.iter().enumerate() {
        if let Some(nm) = &sg.name {
            if let Some(sid) = streets.first(nm.trim()) {
                segs_by_sid.entry(sid).or_default().push(i);
            }
        }
    }

    // Resolved addresses -> number buckets per segment (key VIRT+sid = street without any
    // routable segment — unroutable register street or pseudo-street/village — one synthetic
    // row for the whole element).
    const VIRT: usize = usize::MAX / 2;
    let mut seg_recs: HashMap<usize, Vec<HN>> = HashMap::new();
    let mut virt_cluster: HashMap<u32, u32> = HashMap::new(); // streetless sid -> cluster (see below)
    let mut virt_cnt: HashMap<u32, (i128, i128, u32)> = HashMap::new();
    let mut owned: BTreeSet<u32> = BTreeSet::new(); // streets with at least one numeric address
    let mut nrec_all = 0usize;
    for hit in hits {
        let Some(sid) = streets.pick(&hit.label, hit.coord) else {
            continue;
        };
        let key = match hit.seg {
            Some(i) if segs_by_sid.get(&sid).is_some_and(|l| l.contains(&i)) => i,
            _ => {
                let e = virt_cnt.entry(sid).or_insert((0, 0, 0));
                e.0 += i128::from(hit.coord.0);
                e.1 += i128::from(hit.coord.1);
                e.2 += 1;
                VIRT + sid as usize
            }
        };
        seg_recs.entry(key).or_default().push(hit.hn);
        owned.insert(sid);
        nrec_all += 1;
    }
    if n < 2 || nrec_all == 0 {
        return (0, Default::default());
    } // <2 blocks is invalid for the container; no addresses ⇒ nothing to write
      // Synthetic-row (unroutable street / pseudo-street) cell clusters: prefer the REAL one-cell
      // cluster of the nearest routable segment within 300 m (stock binds village addresses to
      // real road cells, e.g. DEBINY rows -> global cell id 123); purely synthetic id otherwise.
    for (&sid, &(sx, sy, n)) in &virt_cnt {
        let c = (
            sx.div_euclid(n as i128) as i64,
            sy.div_euclid(n as i128) as i64,
        );
        let mut near: Option<(i128, u32)> = None;
        for sg in segs {
            let dd = pt_seg_d2(c, sg.ca, sg.cb);
            if dd <= m2d2(300.0) && near.is_none_or(|(bd, _)| dd < bd) {
                near = Some((dd, sg.cluster));
            }
        }
        virt_cluster.insert(sid, near.map(|(_, cl)| cl).unwrap_or(cluster_next));
    }

    // Always >= 2 blocks tiling [id_lo, id_lo + n).
    let nblk = n.div_ceil(HN_ATTR_CHUNK).max(2);
    let w = n.div_ceil(nblk);
    let mut blocks = Vec::new();
    // sid -> (lowest numeric record, its 0xc11 table row): the PA/destination cell for the street
    // (bGetPACellIDs 00b898dc feeds its cell ids straight into the street block's enGetCells).
    let mut street_cell: BTreeMap<u32, (u32, u32)> = BTreeMap::new();
    for lo in ((id_lo as usize)..(id_lo as usize + n)).step_by(w) {
        let hi = (lo + w).min(id_lo as usize + n);
        let width = (hi - lo) as u32;
        let mut exists = vec![false; width as usize];
        let mut offs: Vec<u32> = Vec::new();
        let mut nums: Vec<u32> = Vec::new();
        let mut tos: Vec<u32> = Vec::new(); // 0xc02 per-record `to` bound (NLHnr+0xc)
        let mut ev: Vec<bool> = Vec::new();
        let mut od: Vec<bool> = Vec::new();
        let mut rec_row: Vec<u32> = Vec::new(); // 0xc11 per-record table row (both sides share one)
        let mut rec_owner: Vec<u32> = Vec::new(); // 0xc11 per-record OWNING STREET element id
        let mut seg_row: HashMap<usize, u32> = HashMap::new(); // block cell-table row per segment
        let mut own_exists = vec![false; width as usize]; // 0x001 exists: street has city owners
        let mut own_offs: Vec<u32> = Vec::new(); // 0x001 value-list starts
        let mut own_ids: Vec<u32> = Vec::new(); // 0x001 per-street owning-city ids (bHasValidOwner ctx)
        let mut row_cluster: Vec<u32> = Vec::new(); // table row -> RNW cluster id (0x004)
        for &sid in owned.range(lo as u32..hi as u32) {
            // rows for this street: its segments that actually carry numbers, else the synthetic
            let mut street_rows: Vec<(usize, u32)> = Vec::new(); // (seg idx | VIRT+sid, cluster)
            if let Some(list) = segs_by_sid.get(&sid) {
                for &i in list {
                    if seg_recs.contains_key(&i) {
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
            let owners: Vec<u32> = street_owners.get(&sid).cloned().unwrap_or_default();
            if !owners.is_empty() {
                own_exists[(sid - lo as u32) as usize] = true;
                own_offs.push(own_ids.len() as u32);
                own_ids.extend(owners.iter().copied());
            }
            for &(seg, cl) in &street_rows {
                let base = match seg_row.get(&seg) {
                    Some(b) => *b,
                    None => {
                        let b = row_cluster.len() as u32;
                        row_cluster.push(cl);
                        seg_row.insert(seg, b);
                        b
                    }
                };
                let Some(list) = seg_recs.get(&seg) else {
                    continue;
                };
                // distinct parsed HNs (1a/1b dedup to one "1" — same building) merged into
                // stock-style records: step-2 consecutive single numbers coalesce into one
                // single-parity range record (stock `5..9` odd), parsed ranges stay as-is
                let list = compose_records(list);
                for hn in list.iter() {
                    nums.push(hn.from);
                    tos.push(hn.to);
                    ev.push(hn.even);
                    od.push(hn.odd);
                    rec_row.push(base);
                    // 0xc11 = the OWNING STREET element id (stock block-8 values, e.g. 957 =
                    // '16 PULKU ULANOW WIELKOPOL., ULICA' in the street list, 20021..20029 =
                    // consecutive 'BARTOSZA GLOWACKIEGO' copies). bHasValidOwner takes it as a
                    // street-set key: the record lives in a city's HNR list iff col 0x001 of that
                    // street carries the city the user selected.
                    rec_owner.push(sid);
                    match street_cell.get(&sid) {
                        Some(&(mn0, _)) if mn0 <= hn.from => {}
                        _ => {
                            street_cell.insert(sid, (hn.from, base));
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
        // 0xc11: per-record OWNING STREET element id (stock block-8: 957, 20021..20029 are
        // street-list ids). `bHasValidOwner 00ce5cf4` keys the selected city's street set with it
        // (set built from col 0x001 city lists) — records of unlinked streets never list.
        let c11 = ColData {
            selector: 0xc11,
            kind: ColKind::ValueList,
            domain: nrec as u32,
            exists: vec![true; nrec],
            counts: (0..nrec as u32).collect(),
            values: rec_owner,
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
            // 0x001: per-street owning-city id list (stock: block-8 col 0x001 VL values are the
            // LID20001 city ids that "own" the street — the city→street list source).
            ColData {
                selector: 0x0001,
                kind: ColKind::ValueList,
                domain: width,
                exists: own_exists,
                counts: own_offs,
                values: own_ids,
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
    let bytes = write_gen_attr_file(id_lo + n as u32, &outer, &blocks);
    if fs::write(path, &bytes).is_err() {
        eprintln!("write {path:?} failed");
        return (0, Default::default());
    }
    (
        nrec_all,
        street_cell
            .into_iter()
            .map(|(sid, (_, row))| (sid, row))
            .collect(),
    )
}

/// `LID30006.DAT` crossings. A crossing = a graph node where at least two distinct street elements
/// meet; the element rows live in the `LID20006` street-element domain (`elem_count` = the name
/// list's element count) and each existing element lists the OTHER street elements that meet it,
/// one entry per distinct meeting node (the stock-DEU shape: self never listed, multi-node
/// partners repeat). [OPEN] positions need the crossing `NLCellIdAttrVector` columns whose
/// semantics are still under RE (LID_format.md §11 note) — this file lists partners only; the
/// `LID4` cell ids are NOT duplicated here.
fn write_crossings(
    path: &Path,
    segs: &[OneCell],
    streets: &SidMap,
    nst: usize,
    region: u16,
) -> usize {
    use lid_format::write::{write_crossing_file, CrossingBlockData};
    use std::collections::{BTreeSet, HashMap};

    if nst < 2 {
        return 0;
    }
    // Node identity = the exact PAU coords of onecell endpoints (shared OSM nodes give bit-equal
    // coords). One pass: node -> sids touching it.
    let mut node_sids: HashMap<(i64, i64), BTreeSet<u32>> = HashMap::new();
    for sg in segs {
        let Some(nm) = &sg.name else { continue };
        let Some(sid) = streets.first(nm.trim()) else {
            continue;
        };
        for end in [sg.ca, sg.cb] {
            node_sids.entry(end).or_default().insert(sid);
        }
    }
    // Junction occurrences in deterministic PAU node order.
    let mut nodes: Vec<(i64, i64)> = node_sids.keys().copied().collect();
    nodes.sort();
    let mut by_sid: BTreeMap<u32, Vec<(i64, i64, u32)>> = BTreeMap::new();
    for node in nodes {
        let sids = &node_sids[&node];
        if sids.len() < 2 {
            continue;
        }
        for &x in sids {
            for &y in sids {
                if x != y {
                    by_sid.entry(x).or_default().push((node.0, node.1, y));
                }
            }
        }
    }
    if by_sid.is_empty() {
        return 0;
    }
    let nblk = nst.div_ceil(HN_ATTR_CHUNK).max(2);
    let w = nst.div_ceil(nblk);
    let mut blocks = Vec::new();
    let mut total_rows = 0usize;
    for lo in (0..nst).step_by(w) {
        let hi = (lo + w).min(nst);
        let mut exists = vec![false; hi - lo];
        let mut starts: Vec<u32> = Vec::new();
        let mut values: Vec<u32> = Vec::new();
        for sid in lo as u32..hi as u32 {
            if let Some(vals) = by_sid.get(&sid) {
                exists[(sid - lo as u32) as usize] = true;
                starts.push(values.len() as u32);
                values.extend(vals.iter().map(|&(_, _, y)| y));
                total_rows += 1;
            }
        }
        blocks.push(CrossingBlockData {
            elem_start: lo as u32,
            elem_end: (hi - 1) as u32,
            exists,
            starts,
            values,
        });
    }
    let outer = lid_format::header::nl_header(lid_format::header::KIND_CROSSING, region, 3, 0);
    let bytes = write_crossing_file(nst as u32, &outer, &blocks);
    if fs::write(path, &bytes).is_err() {
        eprintln!("write {path:?} failed");
        return 0;
    }
    total_rows
}

fn write_pa(
    path: &Path,
    hits: &[AddrHit],
    entries: &[lid_format::NameEntry],
    streets: &SidMap,
    street_cell: &BTreeMap<u32, u32>,
    nst: usize,
    region: u16,
) -> usize {
    use lid_format::pa::{write_pa_file, PaDetailBlock, PaDetailEntry};

    // street element -> lowest numeric address point (coords), if any.
    let mut best: BTreeMap<u32, (u32, (i64, i64))> = BTreeMap::new();
    for hit in hits {
        let Some(sid) = streets.pick(&hit.label, hit.coord) else {
            continue;
        };
        match best.get(&sid) {
            Some(&(bn, _)) if bn <= hit.hn.from => {}
            _ => {
                best.insert(sid, (hit.hn.from, hit.coord));
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
    for (e, &sid) in entries.iter().zip(streets.sid_of_entry.iter()) {
        if sid == u32::MAX || sid as usize >= nst {
            continue;
        }
        if let Some((_, (ax, ay))) = best.get(&sid) {
            // relative PAU offset from the street anchor (the device adds the anchor back).
            let dx = ax - i64::from(e.x_pau);
            let dy = ay - i64::from(e.y_pau);
            if i64::from(i32::MIN) <= dx
                && dx <= i64::from(i32::MAX)
                && i64::from(i32::MIN) <= dy
                && dy <= i64::from(i32::MAX)
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
/// Stock shape (CHECKED): a street carries a pair for EVERY city within 3 km of its reported
/// position — the 12 "DEBINY" village entries hold 44 pairs (avg 3.7/street — through entries span
/// several settlement anchors). The nearest city stays the entry's grouping anchor regardless.
fn write_street_city_rel(
    path: &Path,
    entries: &[lid_format::NameEntry],
    city_of: &[Option<String>],
    sid_of_entry: &[u32],
    city_idx: &HashMap<String, Vec<u32>>,
    city_coords: &std::collections::BTreeMap<&String, (i64, i64)>,
) -> usize {
    let src_elems = sid_of_entry
        .iter()
        .copied()
        .filter(|&s| s != u32::MAX)
        .max()
        .map_or(0, |m| m + 1) as u64;
    let tgt_elems = city_idx
        .values()
        .flatten()
        .max()
        .map_or(0, |&m| m as u64 + 1);
    let mut rels: Vec<(u32, u32)> = Vec::new();
    let mut seen: std::collections::HashSet<(u32, u32)> = std::collections::HashSet::new();
    fn add_pairs(
        s: u32,
        cn: &Option<String>,
        m: &HashMap<String, Vec<u32>>,
        seen: &mut std::collections::HashSet<(u32, u32)>,
        rels: &mut Vec<(u32, u32)>,
    ) {
        if let Some(ids) = cn.as_ref().and_then(|c| m.get(c.as_str())) {
            for &t in ids {
                if seen.insert((s, t)) {
                    rels.push((s, t));
                }
            }
        }
    }
    for ((e, city), &s) in entries.iter().zip(city_of).zip(sid_of_entry) {
        if s == u32::MAX {
            continue;
        }
        add_pairs(s, city, city_idx, &mut seen, &mut rels);
        for (cn, cc) in city_coords {
            if pt_seg_d2((e.x_pau as i64, e.y_pau as i64), *cc, *cc) <= m2d2(3000.0) {
                add_pairs(s, &Some(cn.to_string()), city_idx, &mut seen, &mut rels);
            }
        }
    }
    rels.sort_unstable();
    rels.dedup();
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
    write_file: bool,
) -> (usize, HashMap<String, u32>, Vec<lid_format::NameEntry>) {
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
            belonging: None,
        });
    }
    let bytes = lid_format::encode_id(region, list_id, &entries);
    if write_file && fs::write(path, &bytes).is_err() {
        eprintln!("write {path:?} failed");
        return (0, HashMap::new(), entries);
    }
    let mut idx = HashMap::new();
    if let Ok(nl) = lid_format::read(&bytes) {
        for (i, e) in nl.elements.iter().enumerate() {
            idx.entry(e.name.clone()).or_insert(i as u32);
        }
    }
    (entries.len(), idx, entries)
}

/// Drop the stock city-list postal prefix "NN NNN " from an element name.
fn strip_postal(n: &str) -> &str {
    let b = n.as_bytes();
    if b.len() > 6
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[2] == b' '
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
        && b[5].is_ascii_digit()
        && b[6] == b' '
    {
        &n[7..]
    } else {
        n
    }
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
