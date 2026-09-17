// OSM (PBF or XML) road network -> Bosch TravelMap RNW (NAVnnnnn.DAT clusters).
// This binary owns RNW semantics; the onecell walk + cluster numbering shared with `osm2lid`
// (GenAttr 0x004 global ids must agree) come from the `rnw_model` crate — see its lib docs.
// NAV*.DAT output is byte-deterministic (no RandomState iteration reaches the file).
//
// WRITE-side counterpart of `rnw2osm`. The byte layout is the exact inverse of
// rnw2osm's `parse_cluster`, so output round-trips:
//   osm2rnw in.osm.pbf -o out/            then
//   rnw2osm out/<REGION> -b W,S,E,N -o roundtrip.osm  reproduces roads/names/classes.
//
// Roads only. Every shared road vertex -> a zerocell (node); every consecutive
// vertex pair of a highway way -> one straight onecell (segment). The network is
// split into geographic clusters, each <= 1024 onecells (the DCR ref packs the
// onecell index in 10 bits; > 1024 is unaddressable and would also push a u16
// cluster-relative payload offset past 64 KB).
//
// Cross-cluster continuity: a boundary vertex is duplicated (identical decoded PAU)
// in each cluster that touches it, exactly as Bosch stores it, and its zerocell gets
// the border marker (rim flag). rnw2osm merges these marker-gated duplicates; the CAR
// resolves them via the same border-marker / overlap mechanism. Overlap LINKS naming
// the neighbour onecell (--overlaps) are a further enhancement; see RNW_format §3b.
//
// Format: RNW_format.md §2/§3/§5/§6/§8. Writer guide: writer_guide.md §7.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::exit;

const PAU: f64 = (1i64 << 31) as f64 / 180.0;
const BLOCK: usize = 0x4000; // 16KB cluster alignment (rnw2osm scans on this)
const MAX_OC: usize = 1024; // 10-bit onecell index in DCR/overlap refs
const NAME_STR_FLAG: u8 = 0xA7; // required by the renderer
// Bosch `.tci` descriptive block (bytes 0x14..0x84), verbatim from stock N6E211A.TCI.
const TCI_DESCRIPTOR: [u8; 112] = [
    67, 111, 112, 121, 114, 105, 103, 104, 116, 32, 82, 111,
    98, 101, 114, 116, 45, 66, 111, 115, 99, 104, 45, 71,
    109, 98, 72, 32, 32, 50, 48, 48, 51, 0, 49, 66,
    54, 46, 49, 50, 46, 49, 49, 58, 49, 56, 58, 50,
    48, 0, 84, 73, 76, 69, 95, 67, 76, 85, 83, 84,
    69, 82, 95, 73, 78, 68, 69, 88, 0, 0, 0, 0,
    20, 0, 54, 0, 0, 0, 70, 0, 3, 0, 1, 0,
    0, 0, 49, 66, 54, 46, 49, 50, 46, 49, 49, 58,
    48, 54, 58, 50, 50, 0, 84, 80, 78, 65, 86, 50,
    0, 0, 0, 0,
];

fn deg2pau(d: f64) -> i64 {
    (d * PAU) as i64
}

#[derive(Clone, Default)]
struct Attr {
    rc: u32,
    nc: u32,
    rt: u32,
    link: bool,
    oneway: i8,
    freeway: bool,
}

// highway -> RNW class. Inverts rnw2osm display_class so roads round-trip.
fn classify(hw: &str, junction: &str, oneway: &str) -> Option<Attr> {
    let base = hw.strip_suffix("_link").unwrap_or(hw);
    let is_link = hw.ends_with("_link");
    let (rc, nc, fw) = match base {
        "motorway" => (0, 0, true),
        "trunk" => (1, 1, false),
        "primary" => (1, 3, false),
        "secondary" => (3, 3, false),
        "tertiary" => (4, 7, false),
        "unclassified" | "road" => (6, 7, false),
        "residential" | "living_street" | "service" => (5, 7, false),
        _ => return None,
    };
    let rt = if junction == "roundabout" { 2 } else { 0 };
    let ow = match oneway {
        "yes" | "1" | "true" => 1,
        "-1" | "reverse" => -1,
        _ => 0,
    };
    Some(Attr { rc, nc, rt, link: is_link, oneway: ow, freeway: fw })
}

// ---------------------------------------------------------------------------
// OSM reading (roads only)
// ---------------------------------------------------------------------------
#[derive(Clone)]
struct Seg {
    a: u32,
    b: u32,
    mid: (i64, i64),
    attr: Attr,
    name: Option<String>,
}

struct Network {
    nodes: HashMap<i64, (i64, i64)>,
    ways: Vec<(Vec<i64>, Attr, Option<String>)>,
}
impl Network {
    fn new() -> Self {
        Network { nodes: HashMap::new(), ways: Vec::new() }
    }
}

fn add_way(net: &mut Network, nodes: Vec<i64>, tags: &HashMap<String, String>) {
    if nodes.len() < 2 {
        return;
    }
    let hw = match tags.get("highway") {
        Some(h) => h.as_str(),
        None => return,
    };
    // Road inclusion is the shared rnw_model predicate (the old blacklist here was always
    // redundant with classify()'s acceptance set; rnw_model::routable_way IS that set).
    if !rnw_model::routable_way(hw) {
        return;
    }
    let j = tags.get("junction").map(|s| s.as_str()).unwrap_or("");
    let ow = tags.get("oneway").map(|s| s.as_str()).unwrap_or("");
    let a = match classify(hw, j, ow) {
        Some(a) => a,
        None => return,
    };
    net.ways.push((nodes, a, tags.get("name").cloned()));
}

fn parse_pbf(path: &str, net: &mut Network) {
    use pbf_craft::models::{Element, Tag as PbfTag};
    use pbf_craft::readers::PbfReader;
    fn nd_to_deg(nd: i64) -> f64 {
        if nd < 0 {
            return -nd_to_deg(-nd);
        }
        let whole = nd / 1_000_000_000;
        let frac = nd % 1_000_000_000;
        format!("{}.{:09}", whole, frac).parse::<f64>().unwrap_or_else(|_| nd as f64 / 1e9)
    }
    let mut reader =
        PbfReader::from_path(path).unwrap_or_else(|e| panic!("open pbf {}: {}", path, e));
    reader
        .read(|_h, el| {
            let Some(el) = el else { return };
            match el {
                Element::Node(n) => {
                    if n.visible {
                        net.nodes.insert(
                            n.id,
                            (deg2pau(nd_to_deg(n.longitude)), deg2pau(nd_to_deg(n.latitude))),
                        );
                    }
                }
                Element::Way(w) => {
                    if !w.visible {
                        return;
                    }
                    let ids: Vec<i64> = w.way_nodes.iter().map(|wn| wn.id).collect();
                    let tags: HashMap<String, String> =
                        w.tags.iter().map(|t: &PbfTag| (t.key.clone(), t.value.clone())).collect();
                    add_way(net, ids, &tags);
                }
                Element::Relation(_) => {}
            }
        })
        .unwrap_or_else(|e| panic!("read pbf {}: {}", path, e));
}

fn parse_osm_xml(path: &str, net: &mut Network) {
    use quick_xml::events::Event;
    use std::io::BufReader;
    let file = fs::File::open(path).expect("open osm");
    let mut reader = quick_xml::Reader::from_reader(BufReader::new(file));
    let mut buf = Vec::with_capacity(1 << 16);
    let mut node: Option<(i64, i64, i64)> = None;
    let mut node_tags: HashMap<String, String> = HashMap::new();
    let mut way_ids: Option<Vec<i64>> = None;
    let mut way_tags: HashMap<String, String> = HashMap::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                "node" => {
                    node = Some((0, 0, 0));
                    node_tags.clear();
                    for a in e.attributes().flatten() {
                        match a.key.as_ref() {
                            "id" => node.as_mut().unwrap().0 = str_of(&a).parse().unwrap_or(0),
                            "lon" => node.as_mut().unwrap().1 = deg2pau(str_of(&a).parse().unwrap_or(0.0)),
                            "lat" => node.as_mut().unwrap().2 = deg2pau(str_of(&a).parse().unwrap_or(0.0)),
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
                    net.nodes.insert(id, (lo, la));
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
                    if let Some((id, lo, la)) = node.take() {
                        net.nodes.insert(id, (lo, la));
                    }
                }
                "way" => {
                    if let Some(ids) = way_ids.take() {
                        let tags = std::mem::take(&mut way_tags);
                        add_way(net, ids, &tags);
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

// ---------------------------------------------------------------------------
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut inp: Option<String> = None;
    let mut outp: Option<String> = None;
    let mut region = "POL".to_string();
    let mut file_id: u16 = 20001;
    let mut target_oc: usize = 700;
    let mut bbox: Option<(f64, f64, f64, f64)> = None;
    let mut no_overlaps = false;
    let mut want_tci = false;
    let mut map_idx_dir = String::new();
    let mut region_ident: u16 = 0x402; // placeholder; always derived from --region below unless overridden
    let mut region_ident_set = false;
    let mut tci_file = String::new();
    let mut tci_prof: u16 = 0x1a;

    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-o" | "--out" => {
                outp = Some(args[i + 1].clone());
                i += 2;
            }
            "--region" => {
                region = args[i + 1].clone();
                i += 2;
            }
            "--file-id" => {
                file_id = args[i + 1].parse().unwrap_or(20001);
                i += 2;
            }
            "--target-oc" => {
                target_oc = args[i + 1].parse().unwrap_or(700).min(MAX_OC - 8).max(1);
                i += 2;
            }
            "--bbox" => {
                bbox = parse_bbox(&args[i + 1]);
                i += 2;
            }
            s if s.starts_with("--bbox=") => {
                bbox = parse_bbox(&s[7..]);
                i += 1;
            }
            s if s.starts_with("--out=") => {
                outp = Some(s[6..].to_string());
                i += 1;
            }
            "--no-overlaps" => {
                no_overlaps = true;
                i += 1;
            }
            "--tci" => {
                want_tci = true;
                i += 1;
            }
            "--map-idx" => {
                map_idx_dir = args[i + 1].clone();
                i += 2;
            }
            s if s.starts_with("--map-idx=") => {
                map_idx_dir = s[10..].to_string();
                i += 1;
            }
            "--region-ident" => {
                region_ident = parse_hex_or_dec(&args[i + 1]);
                region_ident_set = true;
                i += 2;
            }
            "--list-region-ids" => {
                print_region_ident_table();
                return;
            }
            "--tci-file" => {
                tci_file = args[i + 1].clone();
                i += 2;
            }
            "--tci-prof" => {
                tci_prof = parse_hex_or_dec(&args[i + 1]);
                i += 2;
            }
            "--help" | "-h" => {
                usage();
                return;
            }
            other => {
                inp = Some(other.to_string());
                i += 1;
            }
        }
    }
    let Some(inp) = inp else {
        usage();
        exit(1);
    };

    // Derive regionIdent from the region code (RNW folder) unless overridden. Unknown code ->
    // require an explicit --region-ident (a wrong value routes to the wrong region on-device).
    if !region_ident_set {
        match region_ident_for(&region) {
            Some(v) => region_ident = v,
            None => {
                eprintln!(
                    "error: no baked regionIdent for region '{}' in the RNW table; pass an\n\
                     \t   explicit --region-ident N, or check codes with --list-region-ids.",
                    region
                );
                exit(1);
            }
        }
    }

    let mut net = Network::new();
    if inp.to_ascii_lowercase().ends_with(".pbf") {
        parse_pbf(&inp, &mut net);
    } else {
        parse_osm_xml(&inp, &mut net);
    }
    eprintln!("parsed: {} nodes, {} road ways", net.nodes.len(), net.ways.len());
    if net.ways.is_empty() {
        eprintln!("no highway ways found");
        exit(1);
    }

    let bb = bbox.map(|(w, s, e, n)| (deg2pau(w), deg2pau(s), deg2pau(e), deg2pau(n)));

    // segments into a compact global node table (index == position in gnodes)
    let mut gnodes: Vec<(i64, i64)> = Vec::new();
    let mut nid: HashMap<i64, u32> = HashMap::new();
    let gid = |nid: &mut HashMap<i64, u32>,
                   gnodes: &mut Vec<(i64, i64)>,
                   nodes: &HashMap<i64, (i64, i64)>,
                   id: i64|
     -> u32 {
        if let Some(&g) = nid.get(&id) {
            return g;
        }
        let g = gnodes.len() as u32;
        gnodes.push(nodes[&id]);
        nid.insert(id, g);
        g
    };
    let mut segs: Vec<Seg> = Vec::new();
    for (ids, attr, name) in &net.ways {
        for pair in ids.windows(2) {
            if !net.nodes.contains_key(&pair[0]) || !net.nodes.contains_key(&pair[1]) {
                continue;
            }
            let a = net.nodes[&pair[0]];
            if let Some((w, s, e, n)) = bb {
                if a.0 < w || a.0 > e || a.1 < s || a.1 > n {
                    continue;
                }
            }
            let ga = gid(&mut nid, &mut gnodes, &net.nodes, pair[0]);
            let gb = gid(&mut nid, &mut gnodes, &net.nodes, pair[1]);
            if ga == gb {
                continue;
            }
            segs.push(Seg {
                a: ga,
                b: gb,
                mid: ((a.0 + net.nodes[&pair[1]].0) / 2, (a.1 + net.nodes[&pair[1]].1) / 2),
                attr: attr.clone(),
                name: name.clone(),
            });
        }
    }
    eprintln!("segments: {}  used nodes: {}", segs.len(), gnodes.len());
    if segs.is_empty() {
        eprintln!("no routable segments in range");
        exit(1);
    }

    let bbox_pau = extent(&gnodes);
    let clusters = build_clusters(&segs, bbox_pau, target_oc);
    eprintln!("clusters: {}", clusters.len());

    let mut blobs = build_blobs(&segs, &gnodes, &clusters);
    mark_borders(&mut blobs, &clusters);
    if !no_overlaps {
        build_overlaps(&mut blobs, &segs);
        let n_ci2: usize = blobs.iter().map(|b| b.ci2.len()).sum();
        let n_ovl: usize = blobs.iter().map(|b| b.oc_ovl.iter().map(|v| v.len()).sum::<usize>()).sum();
        eprintln!("overlap links: ci2 refs={n_ci2}  onecell overlaps={n_ovl}");
    }

    let outp = outp.unwrap_or_else(|| format!("{}_RNW_out", region));
    fs::create_dir_all(Path::new(&outp).join(&region)).expect("mkdir");
    let nav_path = Path::new(&outp).join(&region).join(format!("NAV{:05}.DAT", file_id));
    let cluster_loc = write_nav(&blobs, &segs, &gnodes, file_id, &nav_path);

    let (w, s, e, n) = bbox_pau;
    eprintln!("wrote {} ({} clusters, {} segments)", nav_path.display(), blobs.len(), segs.len());
    eprintln!(
        "validate: rnw2osm {outp}/{region} -b {:.4},{:.4},{:.4},{:.4} -o roundtrip.osm",
        w as f64 / PAU, s as f64 / PAU, e as f64 / PAU, n as f64 / PAU
    );

    if want_tci {
        // Tile grid (region bbox + per-level partition) MUST match osm2map/runtime exactly, so it is
        // read from the step-1 MAP output (a *AA.IDX), NOT from the OSM data extent.
        let (ridx, reg_bbox, _shifts, tilecnt) = match read_map_idx(&map_idx_dir) {
            Some(v) => v,
            None => {
                eprintln!(
                    "error: --tci needs the region tile grid; give --map-idx DIR containing the step-1\n\
                     \t  osm2map <REGION>AA.IDX (or a stock MAP dir). Looked in: '{}'",
                    if map_idx_dir.is_empty() { "<unset>" } else { &map_idx_dir }
                );
                exit(2);
            }
        };
        let file_name = if !tci_file.is_empty() {
            tci_file.clone()
        } else {
            format!("{}1{}", ridx, prof_file_code(tci_prof))
        };
        let map_out = Path::new(&outp).join("MAP");
        fs::create_dir_all(&map_out).expect("mkdir MAP");
        let tci_path = map_out.join(format!("{file_name}.TCI"));
        let (refs, tiles) =
            emit_tci(&tci_path, &blobs, &cluster_loc, reg_bbox, &tilecnt, file_id, region_ident);
        eprintln!(
            "wrote {} ({} tile-refs across {} non-empty tiles; region {} ident 0x{:x}, shard {})",
            tci_path.display(),
            refs,
            tiles,
            ridx,
            region_ident,
            file_name
        );
    } else {
        eprintln!(
            "NOTE (car load): clusters are located at runtime via a per-tile `.tci` cluster-index\n\
            \t  under data/data/map/. Pass --tci --map-idx <step1 MAP out> to emit a matching\n\
            \t  `.tci` (tile -> {{u32 (clusterOffset&~0x3fff)|regionIdent, u16 fileId={file_id}, u16 length}}).\n\
            \t  Cluster header flags byte keeps bit 0x80 CLEAR (osm2rnw flags=0x0001), so stale\n\
            \t  NAV____n.PTH patches are not applied over the clusters."
        );
    }
}

// ---- .tci (TILE_CLUSTER_INDEX) generation -----------------------------------
// Layout reverse-engineered from DAPIAPP.OUT (dap_map_tclTCICache::u16LoadClusterIndexTile /
// u16LoadClusterIdListAndStoreInQ / rnw_tclClusterLoad::u16LoadCluster) and confirmed against stock
// N6E211A.TCI:
//   [0x00] 20B header: u16 f0=0, u16 f1=92, u32 filesize, u16 partOff=0x84, u16 partCnt=4,
//                      u16 122,16,12,106
//   [0x14] 112B descriptive block (verbatim Bosch metadata; identical across shards)
//   [0x84] 4 x TCIPartition(12B) {u8 level, u8 pad[3], u32 maxTile, u32 offsetListOff}
//   [0xb4] per-level tile tables, flat, TCITile(8B) {u16 nPrim,u16 nAll,u32 clusterListOff}
//          indexed at offsetListOff[level] + tileId*8, tileId = region-relative Morton cell index.
//   then the cluster-refs pool: TCIClusterId(8B) {u32 fileOffset, u16 fileId, u16 length} per ref.
// Semantics (routing queries the FINEST level only): u16LoadClusterIdListAndStoreInQ loads a block of
// nAll refs but pushes the first nPrim to the queue, so we write nPrim=nAll=refs.len(). The cluster's
// fileOffset field packs (aligned byte offset & 0xffffc000) | regionIdent (low 14 bits). A cluster is
// registered in EVERY tile (at every level) its bbox overlaps, so a query for any interior point
// resolves it.
fn emit_tci(
    path: &Path,
    blobs: &[ClusterBlob],
    cluster_loc: &[(u32, usize)],
    reg_bbox: (i64, i64, i64, i64),
    tilecnt: &[usize; 4],
    file_id: u16,
    region_ident: u16,
) -> (usize, usize) {
    const PREFIX: usize = 20 + 112 + 4 * 12; // header + descriptive + partition table = 0xb4
    let (wr, sr, er, nr) = reg_bbox;
    let ws = er - wr; // region width  (PAU)
    let hs = nr - sr; // region height (PAU)

    // (level, tileId) -> list of cluster indices overlapping that tile.
    let mut tile_index: [Vec<Vec<usize>>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for lvl in 0..4usize {
        tile_index[lvl] = vec![Vec::new(); tilecnt[lvl]];
    }
    for (ci, cb) in blobs.iter().enumerate() {
        let (w, s, e, n) = cluster_bbox(cb);
        for lvl in 0..4usize {
            let g = grid_size(lvl) as i64;
            let c0 = col_of(w, wr, ws, g);
            let c1 = col_of(e, wr, ws, g);
            let r0 = col_of(s, sr, hs, g);
            let r1 = col_of(n, sr, hs, g);
            for col in c0..=c1 {
                for row in r0..=r1 {
                    let k = cell_to_k(lvl, col, row);
                    if (k as usize) < tilecnt[lvl] {
                        tile_index[lvl][k as usize].push(ci);
                    }
                }
            }
        }
    }

    // Assign pool offsets: build the refs pool per non-empty tile, recording each tile's TCITile.
    let mut pool: Vec<u8> = Vec::new();
    let mut recs: [Vec<(u16, u32)>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()]; // tileId -> (n, poolOff)
    let mut total_refs = 0usize;
    let mut non_empty = 0usize;
    for lvl in 0..4usize {
        for list in tile_index[lvl].iter() {
            if list.is_empty() {
                recs[lvl].push((0, 0));
                continue;
            }
            let off = pool.len() as u32;
            for &ci in list {
                let (foff, len) = cluster_loc[ci];
                pool.extend_from_slice(&(foff & 0xffff_c000 | region_ident as u32).to_le_bytes());
                pool.extend_from_slice(&file_id.to_le_bytes());
                pool.extend_from_slice(&(len as u16).to_le_bytes());
            }
            recs[lvl].push((list.len() as u16, off));
            total_refs += list.len();
            non_empty += 1;
        }
    }

    // Offset-list file offsets (contiguous per level, 8B/tile).
    let mut list_off = [0u32; 4];
    let mut off = PREFIX as u32;
    for lvl in 0..4usize {
        list_off[lvl] = off;
        off += (tilecnt[lvl] as u32) * 8;
    }
    let tables_end = off as usize;
    let filesize = tables_end + pool.len();

    let mut d = vec![0u8; filesize];
    // header
    d[0..2].copy_from_slice(&0u16.to_le_bytes());
    d[2..4].copy_from_slice(&92u16.to_le_bytes());
    d[4..8].copy_from_slice(&(filesize as u32).to_le_bytes());
    d[8..10].copy_from_slice(&0x84u16.to_le_bytes());
    d[10..12].copy_from_slice(&4u16.to_le_bytes());
    d[12..14].copy_from_slice(&122u16.to_le_bytes());
    d[14..16].copy_from_slice(&16u16.to_le_bytes());
    d[16..18].copy_from_slice(&12u16.to_le_bytes());
    d[18..20].copy_from_slice(&106u16.to_le_bytes());
    // descriptive block (verbatim stock)
    d[0x14..0x84].copy_from_slice(&TCI_DESCRIPTOR);
    // partition table
    for lvl in 0..4usize {
        let p = 0x84 + lvl * 12;
        d[p] = lvl as u8;
        d[p + 4..p + 8].copy_from_slice(&(tilecnt[lvl] as u32).to_le_bytes());
        d[p + 8..p + 12].copy_from_slice(&list_off[lvl].to_le_bytes());
    }
    // tile tables
    for lvl in 0..4usize {
        let base = list_off[lvl] as usize;
        for (k, &(cnt, pool_off)) in recs[lvl].iter().enumerate() {
            if cnt == 0 {
                continue;
            }
            let so = base + k * 8;
            d[so..so + 2].copy_from_slice(&cnt.to_le_bytes()); // nPrim
            d[so + 2..so + 4].copy_from_slice(&cnt.to_le_bytes()); // nAll
            let clo = tables_end as u32 + pool_off; // reader only follows this when nAll>0
            d[so + 4..so + 8].copy_from_slice(&clo.to_le_bytes());
        }
    }
    // refs pool
    d[tables_end..].copy_from_slice(&pool);
    fs::write(path, &d).expect("write TCI");
    (total_refs, non_empty)
}

// ---- tile-grid helpers (mirror osm2map, validated byte-exact against runtime) ----
fn grid_size(level: usize) -> usize {
    [1, 5, 50, 500][level]
}
// (col,row) -> tile index K (Morton-style interleave), inverse of osm2map tile_center.
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
// osm2map cell_rect maps col in [0,G) to x in [W + ws*col/G, W + ws*(col+1)/G). Inverse:
fn col_of(x: i64, origin: i64, span: i64, g: i64) -> i64 {
    if span <= 0 {
        return 0;
    }
    let mut c = ((x - origin) as i128 * g as i128 / span as i128) as i64;
    if c < 0 {
        c = 0;
    }
    if c >= g {
        c = g - 1;
    }
    c
}
fn cluster_bbox(cb: &ClusterBlob) -> (i64, i64, i64, i64) {
    let (mut w, mut s, mut e, mut n) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
    for &(x, y) in &cb.outline {
        if x < w { w = x; } if x > e { e = x; } if y < s { s = y; } if y > n { n = y; }
    }
    (w, s, e, n)
}
fn prof_file_code(prof_low: u16) -> String {
    const B: &[u8; 32] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let v = (prof_low & 0xFF) as usize;
    [B[v / 32], B[v % 32]].iter().map(|&c| c as char).collect()
}
fn parse_hex_or_dec(s: &str) -> u16 {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u16::from_str_radix(h, 16).unwrap_or(0)
    } else {
        t.parse().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// RNW routing-region ident table: region code (RNW folder under DATA/DATA/RNW/CCP/)
// -> the 14-bit regionIdent packed into every .tci clusterRef fileOffset low word.
// regionIdent = (profile<<10)|codeId; every shipped region uses profile 1 (CCP), so
// all values are 0x400|codeId. Derived from the stock data, not guessed:
//   * primary: each region's regionIdent repeats ~1e3 times in its own NAV_ROOT.DAT but
//     ~1 elsewhere -> NAV_ROOT histogram with owner-vs-foreign margin (>=8x) — cross-checked
//     against the constant regionIdent inside that region's stock .TCI cluster-index shards;
//   * BNL/MLC are small/overlay regions with no dominant NAV_ROOT signal; they are the two
//     values left over from the confirmed 17-value set {1,2,3,4,6,7,8,9,10,11,12,13,14,17,18,
//     22,42} once the other 15 are pinned, so BNL=0x404 (huge dense Benelux .TCI shards) and
//     MLC=0x40a (tiny counters). GRC=0x406 is NAV_ROOT-clean (83% share) though no stock .TCI
//     surfaced it. Poland's road-network region is POL (0x402): rnw2osm on CCP/POL yields ~5888 roads
//     near Krakow (krzeszowice) while CCP/EEU yields 0, and stock shard N6E2102.TCI references those
//     clusters with a uniform regionIdent 0x402. EEU (0x42a) is a separate aggregate region sharing the
//     same MAP grid (shard N6E211A.TCI) and does NOT carry Poland's roads.
// Confidence in the codeId column: high except BNL/MLC (elimination) — override with
// --region-ident when targeting a region whose stock value is not yet verified on-device.
const REGION_IDENT: &[(&str, u16)] = &[
    ("DEU", 0x401), // 1   Germany
    ("POL", 0x402), // 2   Poland          (road network lives in CCP/POL; shard N6E2102 -> 0x402)
    ("FRM", 0x403), // 3   France
    ("BNL", 0x404), // 4   Benelux        (by elimination)
    ("GRC", 0x406), // 6   Greece
    ("TUR", 0x407), // 7   Turkey
    ("ACL", 0x408), // 8   Austria/Aachen
    ("IBE", 0x409), // 9   Great Britain
    ("MLC", 0x40a), // 10  (small overlay region) (by elimination)
    ("ISV", 0x40b), // 11  Switzerland
    ("GBI", 0x40c), // 12  Great Britain/Ireland
    ("SCA", 0x40d), // 13  Scandinavia
    ("CHS", 0x40e), // 14  (central/eastern)
    ("ELL", 0x411), // 17
    ("INT", 0x412), // 18
    ("EAD", 0x416), // 22
    ("EEU", 0x42a), // 42  Eastern Europe  (a SEPARATE aggregate region; NOT where Poland's roads sit)
];

// region code -> regionIdent (case-insensitive, matches the RNW folder name).
fn region_ident_for(code: &str) -> Option<u16> {
    let up = code.trim().to_ascii_uppercase();
    REGION_IDENT.iter().find(|(k, _)| *k == up).map(|(_, v)| *v)
}

fn print_region_ident_table() {
    println!("RNW regionIdent table (region code -> regionIdent, profile 1 / CCP):");
    for (k, v) in REGION_IDENT {
        println!(
            "  {:<4} 0x{:04x}  (codeId {})",
            k,
            v,
            v & 0x3ff
        );
    }
}
// Read a *AA.IDX (step-1 osm2map output or a stock MAP dir) for the region tile grid.
fn read_map_idx(dir: &str) -> Option<(String, (i64, i64, i64, i64), [i32; 4], [usize; 4])> {
    let mut found: Option<std::path::PathBuf> = None;
    if let Ok(rd) = fs::read_dir(dir) {
        for ent in rd.flatten() {
            let nm = ent.file_name().to_string_lossy().to_string();
            if nm.ends_with("AA.IDX") {
                found = Some(ent.path());
                break;
            }
        }
    }
    let p = found?;
    let d = fs::read(&p).ok()?;
    if d.len() < 0x20 {
        return None;
    }
    let stem = p.file_name()?.to_string_lossy().to_string();
    let ridx = stem.trim_end_matches("AA.IDX").to_string();
    let g32 = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]) as i32 as i64;
    let bbox = (g32(4), g32(8), g32(12), g32(16));
    let part_off = u16::from_le_bytes([d[0x14], d[0x15]]) as usize * 4;
    let mut shifts = [0i32; 4];
    let mut tilecnt = [0usize; 4];
    for lvl in 0..4usize {
        let o = part_off + lvl * 12;
        if o + 7 >= d.len() {
            return None;
        }
        let u32a = u32::from_le_bytes([d[o + 3], d[o + 4], d[o + 5], d[o + 6]]);
        shifts[lvl] = (u32a & 0xff) as i32;
        tilecnt[lvl] = (u32a >> 8) as usize;
    }
    Some((ridx, bbox, shifts, tilecnt))
}

fn parse_bbox(s: &str) -> Option<(f64, f64, f64, f64)> {
    let p: Vec<f64> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if p.len() == 4 && p[0] < p[2] && p[1] < p[3] {
        Some((p[0], p[1], p[2], p[3]))
    } else {
        None
    }
}
fn extent(g: &[(i64, i64)]) -> (i64, i64, i64, i64) {
    rnw_model::extent(g)
}

fn build_clusters(segs: &[Seg], bbox: (i64, i64, i64, i64), target: usize) -> Vec<Vec<usize>> {
    // THE clusterer lives in rnw_model — shared verbatim with osm2lid so `0x004` global ids
    // agree across a same-source RNW+LID build without either tool reading the other (§11.6b).
    let mids: Vec<(i64, i64)> = segs.iter().map(|s| s.mid).collect();
    rnw_model::build_clusters(&mids, bbox, target)
}

struct ClusterBlob {
    origin: (i64, i64),
    shift: i32,
    outline: Vec<(i64, i64)>,
    local_nodes: Vec<u32>,
    node_of_global: HashMap<u32, u16>,
    oc_from: Vec<u16>,
    oc_to: Vec<u16>,
    oc_seg: Vec<usize>,
    node_border: Vec<bool>,
    // ci2 overlap links (filled by build_overlaps; serialized as cluster bit3 + onecell bit4).
    ci2: Vec<usize>,                            // neighbour cluster indices, in ci2-slot order
    oc_ovl: Vec<Vec<(u16, u16, usize)>>,        // per onecell: (ci2_index, nbr onecell local, nbr cluster)
}

fn build_blobs(segs: &[Seg], gnodes: &[(i64, i64)], clusters: &[Vec<usize>]) -> Vec<ClusterBlob> {
    clusters
        .iter()
        .map(|seg_idx| {
            let mut node_of_global: HashMap<u32, u16> = HashMap::new();
            let mut local_nodes: Vec<u32> = Vec::new();
            for &si in seg_idx {
                for g in [segs[si].a, segs[si].b] {
                    if !node_of_global.contains_key(&g) {
                        node_of_global.insert(g, local_nodes.len() as u16);
                        local_nodes.push(g);
                    }
                }
            }
            let (mut w, mut e, mut s, mut n) = (i64::MAX, i64::MIN, i64::MAX, i64::MIN);
            for &g in &local_nodes {
                let (x, y) = gnodes[g as usize];
                if x < w { w = x; } if x > e { e = x; } if y < s { s = y; } if y > n { n = y; }
            }
            let mut shift = 0i32;
            while shift < 13 && ((e - w).max(n - s) >> shift) > 32000 {
                shift += 1;
            }
            let oc_from = seg_idx.iter().map(|&si| node_of_global[&segs[si].a]).collect();
            let oc_to = seg_idx.iter().map(|&si| node_of_global[&segs[si].b]).collect();
            ClusterBlob {
                origin: ((w + e) / 2, (s + n) / 2),
                shift,
                outline: vec![(w, s), (e, s), (e, n), (w, n)],
                local_nodes,
                node_of_global,
                oc_from,
                oc_to,
                oc_seg: seg_idx.clone(),
                node_border: Vec::new(),
                ci2: Vec::new(),
                oc_ovl: vec![Vec::new(); seg_idx.len()],
            }
        })
        .collect()
}

// Cross-cluster overlap links (byte-faithful stitch). For every vertex shared by more than
// one cluster, link the copies two ways through ci2: cluster -> its "home" cluster (first
// cluster owning the vertex) and home -> that cluster. `rnw2osm`'s Phase C resolves each ref
// by origin (the ci2 record's refLon/refLat) and unions the two endpoint nodes; the runtime
// follows the same link (bRelevantCrossingBetween). One ref per (cluster-pair, vertex) is
// enough — the node union shares the junction between every onecell that touches it.
fn build_overlaps(blobs: &mut [ClusterBlob], segs: &[Seg]) {
    // global node -> ordered, distinct list of clusters touching it (first = home).
    let mut owners: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut rep: HashMap<(usize, u32), u16> = HashMap::new(); // (cluster, node) -> representative onecell
    for b in 0..blobs.len() {
        for oi in 0..blobs[b].oc_seg.len() {
            for lnode in [blobs[b].oc_from[oi], blobs[b].oc_to[oi]] {
                let g = blobs[b].local_nodes[lnode as usize];
                if !rep.contains_key(&(b, g)) {
                    rep.insert((b, g), oi as u16);
                    owners.entry(g).or_default().push(b);
                }
            }
        }
    }

    let add_link = |dst: usize,
                    dst_oc: u16,
                    nbr: usize,
                    nbr_oc: u16,
                    blobs: &mut [ClusterBlob]| {
        let ci2i = match blobs[dst].ci2.iter().position(|&c| c == nbr) {
            Some(i) => i,
            None => {
                blobs[dst].ci2.push(nbr);
                blobs[dst].ci2.len() - 1
            }
        };
        blobs[dst].oc_ovl[dst_oc as usize].push((ci2i as u16, nbr_oc, nbr));
    };

    // Sort the shared-node keys: ci2/oc_ovl record order follows THIS iteration, and the
    // HashMap order is per-process random — unsorted keys made NAV*.DAT non-reproducible.
    let mut shared_nodes: Vec<u32> = owners.keys().copied().collect();
    shared_nodes.sort_unstable();
    for g in shared_nodes {
        let own = owners[&g].clone();
        if own.len() < 2 {
            continue;
        }
        let home = own[0];
        let home_oc = rep[&(home, g)];
        for &c in &own[1..] {
            let c_oc = rep[&(c, g)];
            add_link(c, c_oc, home, home_oc, blobs);
            add_link(home, home_oc, c, c_oc, blobs);
        }
    }
    let _ = segs;
}

// A node shared by >1 cluster is a boundary junction: set its border (rim) flag in
// every cluster that holds it, so both readers stitch the duplicates.
fn mark_borders(blobs: &mut [ClusterBlob], _clusters: &[Vec<usize>]) {
    let mut border: Vec<Vec<bool>> = blobs.iter().map(|b| vec![false; b.local_nodes.len()]).collect();
    let mut owner: HashMap<u32, usize> = HashMap::new();
    for ci in 0..blobs.len() {
        for idx in 0..blobs[ci].local_nodes.len() {
            let g = blobs[ci].local_nodes[idx];
            match owner.get(&g) {
                Some(&first) if first != ci => {
                    border[ci][idx] = true;
                    let lo = blobs[first].node_of_global[&g] as usize;
                    if lo < border[first].len() {
                        border[first][lo] = true;
                    }
                }
                Some(_) => {}
                None => {
                    owner.insert(g, ci);
                }
            }
        }
    }
    for b in 0..blobs.len() {
        blobs[b].node_border = std::mem::take(&mut border[b]);
    }
}

fn d3(v: i64, shift: i32) -> i16 {
    let r = if v >= 0 { 1i64 << shift >> 1 } else { -(1i64 << shift >> 1) };
    ((v + r) >> shift).clamp(-32767, 32767) as i16
}

fn serialize(
    cb: &ClusterBlob,
    blobs: &[ClusterBlob],
    gnodes: &[(i64, i64)],
    segs: &[Seg],
    cluster_id: u16,
) -> (Vec<u8>, Vec<usize>) {
    let zc = cb.local_nodes.len();
    let oc = cb.oc_seg.len();
    let shift = cb.shift;

    let ooff = 0x1Ausize;
    let desc_at = ooff + 4 * cb.outline.len();
    let has_ci2 = !cb.ci2.is_empty();
    let desc_slots = 3 + if has_ci2 { 1 } else { 0 }; // ci2(3) + zero(4) + one(5) + pos(8)
    let desc_len = 4 * desc_slots;
    let pos_off = desc_at + desc_len;
    let zero_off = pos_off + 4 * zc;
    let one_off = zero_off + 6 * zc;
    let mut var = one_off + 12 * oc;

    let mut node_dcr: Vec<Vec<u16>> = vec![Vec::new(); zc];
    for oi in 0..oc {
        node_dcr[cb.oc_from[oi] as usize].push(((oi + 1) as u16) & 0x3FF);
        node_dcr[cb.oc_to[oi] as usize].push((((oi + 1) as u16) & 0x3FF) | 0x8000);
    }
    let mut dcr_desc = vec![0u16; zc];
    let mut dcr_data = vec![0u16; zc];
    for i in 0..zc {
        dcr_desc[i] = var as u16;
        var += 4;
        dcr_data[i] = var as u16;
        var += 2 * node_dcr[i].len();
    }

    // ci2 neighbour-cluster records (24 B each): num@u16+4, origin lon/lat @i32+8/+12; the
    // fileOffset(u32@0)/fileId(u16@6) are patched by write_nav once cluster offsets are fixed.
    let ci2_off = var;
    var += 24 * cb.ci2.len();

    // per-onecell: overlap entries + name flag + descriptor stream (bit0 name, bit4 overlaps).
    let mut has_name = vec![false; oc];
    let mut ovl_cnt = vec![0u16; oc];
    for oi in 0..oc {
        has_name[oi] = segs[cb.oc_seg[oi]].name.is_some();
        ovl_cnt[oi] = cb.oc_ovl[oi].len() as u16;
    }
    // descriptor stream location (at onecell offf): one 4-byte slot per set bit, in bit order.
    let mut oc_desc = vec![0u16; oc];
    for oi in 0..oc {
        let nslots = (has_name[oi] as usize) + if ovl_cnt[oi] > 0 { 1 } else { 0 };
        if nslots > 0 {
            oc_desc[oi] = var as u16;
            var += 4 * nslots;
        }
    }
    let mut oc_ann = vec![0u16; oc];
    for oi in 0..oc {
        if has_name[oi] {
            oc_ann[oi] = var as u16;
            var += 6;
        }
    }
    let mut oc_ovl_arr = vec![0u16; oc];
    for oi in 0..oc {
        if ovl_cnt[oi] > 0 {
            oc_ovl_arr[oi] = var as u16;
            var += 4 * ovl_cnt[oi] as usize;
        }
    }
    let mut name_text: HashMap<&str, u16> = HashMap::new();
    for oi in 0..oc {
        if let Some(nm) = segs[cb.oc_seg[oi]].name.as_deref() {
            if !name_text.contains_key(nm) {
                let o = var;
                var += 3 + nm.len();
                name_text.insert(nm, o as u16);
            }
        }
    }
    let mut b = vec![0u8; var];

    put_u16(&mut b, 0x00, cluster_id.max(1));
    put_u16(&mut b, 0x02, 0x0001);
    put_u32(&mut b, 0x04, 0);
    put_i32(&mut b, 0x08, cb.origin.0 as i32);
    put_i32(&mut b, 0x0c, cb.origin.1 as i32);
    b[0x10] = shift as u8;
    put_u16(&mut b, 0x12, ooff as u16);
    put_u16(&mut b, 0x14, cb.outline.len() as u16);
    let mut lf: u16 = (1 << 4) | (1 << 5) | (1 << 8);
    if has_ci2 {
        lf |= 1 << 3;
    }
    put_u16(&mut b, 0x16, lf);
    put_u16(&mut b, 0x18, 0);

    for (i, &(x, y)) in cb.outline.iter().enumerate() {
        put_u16(&mut b, ooff + i * 4, d3(x - cb.origin.0, shift) as u16);
        put_u16(&mut b, ooff + i * 4 + 2, d3(y - cb.origin.1, shift) as u16);
    }
    // descriptor slots in ascending-bit order: [ci2] zero one pos
    let mut d = desc_at;
    if has_ci2 {
        put_u16(&mut b, d, ci2_off as u16);
        put_u16(&mut b, d + 2, cb.ci2.len() as u16);
        d += 4;
    }
    put_u16(&mut b, d, zero_off as u16);
    put_u16(&mut b, d + 2, zc as u16);
    put_u16(&mut b, d + 4, one_off as u16);
    put_u16(&mut b, d + 6, oc as u16);
    put_u16(&mut b, d + 8, pos_off as u16);
    put_u16(&mut b, d + 10, zc as u16);

    // ci2 records: num + origin now; fileOffset/fileId patched later by write_nav.
    let mut ci2_sites = Vec::with_capacity(cb.ci2.len());
    for (k, &tgt) in cb.ci2.iter().enumerate() {
        let rec = ci2_off + k * 24;
        put_u16(&mut b, rec + 4, (tgt as u16) + 1);
        put_i32(&mut b, rec + 8, blobs[tgt].origin.0 as i32);
        put_i32(&mut b, rec + 12, blobs[tgt].origin.1 as i32);
        ci2_sites.push(rec);
    }

    for (i, &g) in cb.local_nodes.iter().enumerate() {
        let (x, y) = gnodes[g as usize];
        put_u16(&mut b, pos_off + i * 4, d3(x - cb.origin.0, shift) as u16);
        put_u16(&mut b, pos_off + i * 4 + 2, d3(y - cb.origin.1, shift) as u16);
    }
    for i in 0..zc {
        let p = zero_off + i * 6;
        put_u16(&mut b, p, if cb.node_border.get(i).copied().unwrap_or(false) { 0x2 } else { 0 });
        put_u16(&mut b, p + 2, 0x02);
        put_u16(&mut b, p + 4, dcr_desc[i]);
        put_u16(&mut b, dcr_desc[i] as usize, dcr_data[i]);
        put_u16(&mut b, dcr_desc[i] as usize + 2, node_dcr[i].len() as u16);
        for (j, &v) in node_dcr[i].iter().enumerate() {
            put_u16(&mut b, dcr_data[i] as usize + j * 2, v);
        }
    }
    for oi in 0..oc {
        let p = one_off + oi * 12;
        let si = cb.oc_seg[oi];
        let s = &segs[si];
        let mut hdr: u32 = 0;
        hdr |= s.attr.rc & 7;
        hdr |= (s.attr.nc & 7) << 4;
        hdr |= (s.attr.rt & 0xF) << 8;
        if s.attr.link { hdr |= 1 << 13; }
        if s.attr.oneway > 0 { hdr |= 1 << 20; } else if s.attr.oneway < 0 { hdr |= 1 << 21; }
        if s.attr.freeway { hdr |= 1 << 30; }
        put_u32(&mut b, p, hdr);
        put_u32(&mut b, p + 4, (seg_len(gnodes, s) as u32) & 0x00FF_FFFF);
        let mut lfo: u16 = 0;
        if has_name[oi] {
            lfo |= 1;
        }
        if ovl_cnt[oi] > 0 {
            lfo |= 1 << 4;
        }
        put_u16(&mut b, p + 8, lfo);
        put_u16(&mut b, p + 10, if lfo != 0 { oc_desc[oi] } else { 4 });
        if lfo != 0 {
            let mut q = oc_desc[oi] as usize;
            if has_name[oi] {
                put_u16(&mut b, q, oc_ann[oi]);
                put_u16(&mut b, q + 2, 1);
                q += 4;
                put_u16(&mut b, oc_ann[oi] as usize, 6);
                put_u16(&mut b, oc_ann[oi] as usize + 2, 0x3C);
                let nm = segs[si].name.clone().unwrap();
                put_u16(&mut b, oc_ann[oi] as usize + 4, name_text[nm.as_str()]);
            }
            if ovl_cnt[oi] > 0 {
                put_u16(&mut b, q, oc_ovl_arr[oi]);
                put_u16(&mut b, q + 2, ovl_cnt[oi]);
                for (j, &(ci2i, nbr_oc, _)) in cb.oc_ovl[oi].iter().enumerate() {
                    let r = oc_ovl_arr[oi] as usize + j * 4;
                    put_u16(&mut b, r, (nbr_oc + 1) & 0x3FF); // neighbour onecell index + 1 (10 bits)
                    put_u16(&mut b, r + 2, ci2i + 1); // ci2 index + 1
                }
            }
        }
    }
    for (nm, off) in name_text.iter() {
        let o = *off as usize;
        b[o] = 1;
        b[o + 1] = NAME_STR_FLAG;
        b[o + 2] = nm.len() as u8;
        b[o + 3..o + 3 + nm.len()].copy_from_slice(nm.as_bytes());
    }
    (b, ci2_sites)
}

fn seg_len(g: &[(i64, i64)], s: &Seg) -> f64 {
    let (x0, y0) = g[s.a as usize];
    let (x1, y1) = g[s.b as usize];
    let dx = (x1 - x0) as f64 * 180.0 / (1i64 << 31) as f64 * 111320.0;
    let dy = (y1 - y0) as f64 * 180.0 / (1i64 << 31) as f64 * 111320.0 * (y0 as f64 / PAU).to_radians().cos();
    (dx * dx + dy * dy).sqrt().max(1.0)
}

fn write_nav(
    blobs: &[ClusterBlob],
    segs: &[Seg],
    gnodes: &[(i64, i64)],
    file_id: u16,
    path: &Path,
) -> Vec<(u32, usize)> {
    // Pass A: serialize each cluster (sizes independent of cross-cluster offsets), assign each
    // cluster a 16KB-aligned file offset.
    let mut made: Vec<(Vec<u8>, Vec<usize>)> = Vec::with_capacity(blobs.len());
    let mut offs: Vec<u32> = Vec::with_capacity(blobs.len());
    let mut off = BLOCK as u32;
    for (i, cb) in blobs.iter().enumerate() {
        let (b, sites) = serialize(cb, blobs, gnodes, segs, (i as u16) + 1);
        assert!(b.len() < 0xFF00, "cluster {} blob {} > 64KB (split with smaller --target-oc)", i, b.len());
        assert!(cb.oc_seg.len() <= MAX_OC);
        offs.push(off);
        off = (off as usize + b.len()) as u32;
        off = ((off as usize + BLOCK - 1) / BLOCK * BLOCK) as u32; // pad to next 16KB
        made.push((b, sites));
    }
    // Pass B: patch each ci2 record's fileOffset(u32@0)+fileId(u16@6) with the neighbour's
    // real file position. num + origin were written in serialize.
    for (i, (b, sites)) in made.iter_mut().enumerate() {
        for (k, &site) in sites.iter().enumerate() {
            let tgt = blobs[i].ci2[k];
            put_u32(b, site, offs[tgt]);
            put_u16(b, site + 6, file_id);
        }
    }
    // Assemble: cluster i at offs[i], zero-padded, first cluster starting at the 16KB mark.
    let end = offs
        .last()
        .map(|&o| (o as usize + made.last().unwrap().0.len() + BLOCK - 1) / BLOCK * BLOCK)
        .unwrap_or(BLOCK);
    let mut file = vec![0u8; end];
    for (i, (b, _)) in made.iter().enumerate() {
        let s = offs[i] as usize;
        file[s..s + b.len()].copy_from_slice(b);
    }
    fs::write(path, &file).expect("write NAV");
    // Return each cluster's (16KB-aligned fileOffset, byte length) for the .tci locator.
    offs.into_iter()
        .zip(made.iter().map(|(b, _)| b.len()))
        .collect()
}

fn put_u16(b: &mut [u8], o: usize, v: u16) {
    b[o..o + 2].copy_from_slice(&v.to_le_bytes());
}
fn put_u32(b: &mut [u8], o: usize, v: u32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_i32(b: &mut [u8], o: usize, v: i32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

fn usage() {
    eprintln!(
        "osm2rnw — OSM road network (PBF/XML) -> Bosch RNW NAV*.DAT clusters (+ optional .tci locator)\n\
         usage: osm2rnw <in.osm.pbf|in.osm> [-o OUTDIR] [--region NAME] [--file-id N]\n\
         \t\t [--target-oc N] [--bbox W,S,E,N] [--no-overlaps] [--tci --map-idx DIR [--region-ident N] [--tci-prof HEX|--tci-file NAME]]\n\
         \t-o             output dir (default <REGION>_RNW_out): <out>/<REGION>/NAV<file_id>.DAT\n\
          \t--region       RNW region folder (default POL); drives the regionIdent unless overridden\n\
          \t--file-id      NAV file id (default 20001)\n\
          \t--target-oc    segments/cluster <=1024 (default 700)\n\
          \t--bbox         only roads inside W,S,E,N degrees (default: input extent)\n\
          \t--no-overlaps  skip ci2 overlap links (border markers only; default emits ci2)\n\
          \t--tci          also emit the tile->cluster locator <out>/MAP/<shard>.TCI\n\
          \t--map-idx      DIR with the step-1 osm2map <REGION>AA.IDX (source of the region tile grid)\n\
          \t--region-ident override the ref regionIdent (default: derived from --region via baked table)\n\
          \t--list-region-ids  print the RNW region-code -> regionIdent table and exit\n\
          \t--tci-prof     shard profile code for the .tci file name (default 0x1a -> region1..)\n\
         \t--tci-file     explicit .tci shard base name (overrides <mapregion>1<code>)\n\
         validate: rnw2osm <out>/<REGION> -b W,S,E,N -o roundtrip.osm"
    );
}

