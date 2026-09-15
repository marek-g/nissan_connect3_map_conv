// lid2dump — Bosch TravelMap LID (address / POI / city) -> JSON dumper.
//
// Read-side ground truth for validating `osm2lid`. It decodes LID files into a
// diffable JSON gazetteer:
//   * SQLite files (GLOB_POI.DAT, DB_CITY.DAT)  -> every table's rows as JSON objects
//   * binary LID*.DAT / REL / PA                -> CPRNAV-decompress (if compressed),
//                                                  then an ASF header summary + every
//                                                  embedded ASCII string (the name-list
//                                                  text pool: `NAME/CODE/COUNTRY`, `ULICA …`).
//
// The columnar name->coordinate mapping (LID_format.md §11) is intentionally left as a
// structural dump: strings + header + region id. That is enough to diff a writer's output
// against stock now; per-element coordinate columns are a later refinement.
//
// Format refs: LID_format.md §3 (GLOB_POI schema), §10 (OSDE sources), §11 (ASF framing).

use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::exit;

use serde_json::{json, Map, Value};

use lid2dump::sqlite_export::{self, Cell, ExportElement, ExportFile, MirrorTable, RelOut};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut inputs: Vec<String> = Vec::new();
    let mut out: Option<String> = None;
    let mut sqlite_out: Option<String> = None;
    let mut country: Option<String> = None;
    let mut coords_mode: String = "auto".into();
    let mut recursive = false;
    let mut no_strings = false;
    let mut exact = false;
    let mut filters: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                i += 1;
                out = args.get(i).cloned();
            }
            "--sqlite" => {
                i += 1;
                sqlite_out = args.get(i).cloned();
            }
            "--country" => {
                i += 1;
                country = args.get(i).cloned();
            }
            "--coords" => {
                i += 1;
                coords_mode = args.get(i).cloned().unwrap_or_else(|| "auto".into());
            }
            "-r" | "--recursive" => recursive = true,
            "--no-strings" => no_strings = true,
            "--exact" => exact = true,
            "--filter" => {
                i += 1;
                filters.extend(
                    args.get(i)
                        .map(|s| s.split(',').map(|p| p.to_lowercase()).collect::<Vec<_>>())
                        .unwrap_or_default(),
                );
            }
            "-h" | "--help" => {
                usage();
                exit(0);
            }
            other => inputs.push(other.to_string()),
        }
        i += 1;
    }
    if inputs.is_empty() {
        usage();
        exit(1);
    }

    let mut files: Vec<Value> = Vec::new();
    let mut exports: Vec<ExportFile> = Vec::new();
    let mut targets: Vec<PathBuf> = Vec::new();
    for inp in &inputs {
        let p = Path::new(inp);
        if p.is_dir() {
            collect(p, recursive, &mut targets);
        } else if p.is_file() {
            targets.push(p.to_path_buf());
        } else {
            eprintln!("warning: no such path: {inp}");
        }
    }

    for t in &targets {
        match dump_file(t, &filters, no_strings, exact) {
            Ok(v) => files.push(v),
            Err(e) => {
                eprintln!("error: {}: {e}", t.display());
                files.push(json!({"path": t.display().to_string(), "error": e}));
            }
        }
        if sqlite_out.is_some() {
            match decode_for_export(t, country.as_deref(), &coords_mode) {
                Ok(Some(ex)) => exports.push(ex),
                Ok(None) => {}
                Err(e) => eprintln!("export-skip {}: {e}", t.display()),
            }
        }
    }
    if let Some(p) = &sqlite_out {
        match sqlite_export::export(Path::new(p), &exports) {
            Ok(()) => {
                eprintln!(
                    "wrote {p} ({} files, {} elements, {} rel pairs, {} hnr rows)",
                    exports.len(),
                    exports.iter().map(|f| f.elements.len()).sum::<usize>(),
                    exports
                        .iter()
                        .map(|f| f.rel.as_ref().map_or(0, |r| r.pairs.len()))
                        .sum::<usize>(),
                    exports.iter().map(|f| f.hnr.len()).sum::<usize>()
                );
            }
            Err(e) => {
                eprintln!("error: sqlite export {p}: {e}");
                exit(1);
            }
        }
    }

    let doc = json!({"files": files});
    let text = serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string());
    match out {
        Some(o) => {
            fs::write(&o, text).unwrap_or_else(|e| {
                eprintln!("cannot write {o}: {e}");
                exit(1);
            });
            eprintln!("wrote {}", o);
        }
        None => {
            let so = io::stdout();
            let mut h = so.lock();
            let _ = h.write_all(text.as_bytes());
            let _ = h.write_all(b"\n");
        }
    }
}

fn usage() {
    eprintln!(
        "Usage: lid2dump [opts] <file-or-dir>...\n\
        \t-o FILE          write JSON here (default stdout)\n\
        \t--sqlite OUT.db  also write one relational SQLite db (tables file/element/rel/hnr,\n\
        \t                   views v_element/v_city/v_street/v_address/v_completeness,\n\
        \t                   bundled SELECTs in table `queries`)\n\
        \t--country XXX    country label for the export (default: path segment after CCP/)\n\
        \t--coords MODE    abs lat/lon model: auto (single-block+origin), file-origin\n\
        \t                   (one anchor per file, e.g. osm2lid output), none (default auto)\n\
        \t-r, --recursive  recurse into directories\n\
        \t--no-strings     skip the raw ASCII string pool (smaller JSON)\n\
        \t--filter a,b,c   keep name-list elements whose name contains any of these (case-insensitive)\n\
	--exact           with --filter: match full name instead of substring\n\
        \nDecodes GLOB_POI.DAT / DB_CITY.DAT (SQLite) and LID*/REL* (binary, canonical 0x77-header,\n\
        name-lists, GenAttr incl. env LID2DUMP_BLOCKS=<n|all> deep column decode, REL pair matrices).\n\
        \tAccepts a whole .../LID/CCP/<REGION>/ directory."
    );
}

/// Decode one input into the relational export model (independent of the JSON path).
/// Returns Ok(None) for files that carry no exportable structure.
fn decode_for_export(
    path: &Path,
    country_flag: Option<&str>,
    coords_mode: &str,
) -> Result<Option<ExportFile>, String> {
    let data = fs::read(path).map_err(|e| format!("read: {e}"))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "input".to_string());
    let country = country_flag.map(|c| c.to_string()).or_else(|| {
        let comps: Vec<String> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .collect();
        comps
            .iter()
            .position(|c| c.eq_ignore_ascii_case("CCP"))
            .and_then(|i| comps.get(i + 1).cloned())
            .filter(|c| !c.is_empty())
    });
    let mk = |kind: &str| ExportFile {
        path: path.display().to_string(),
        name: stem.clone(),
        kind: kind.to_string(),
        list_id: None,
        country: country.clone(),
        block_count: None,
        origin: None,
        coords_valid_override: None,
        elements: Vec::new(),
        rel: None,
        hnr: Vec::new(),
        mirrors: Vec::new(),
    };

    if data.starts_with(b"SQLite format 3") {
        let mut ex = mk("sqlite");
        let db = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| format!("sqlite open: {e}"))?;
        let mut stmt = db
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .map_err(|e| e.to_string())?;
        let tables: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        drop(stmt);
        for name in tables {
            if name.contains("_content")
                || name.contains("_segments")
                || name.contains("_segdir")
                || name.contains("_docsize")
                || name.contains("_stat")
            {
                continue;
            }
            let mut cols: Vec<String> = Vec::new();
            if let Ok(mut s) = db.prepare(&format!("PRAGMA table_info(\"{name}\")")) {
                if let Ok(it) = s.query_map([], |r| r.get::<_, String>(1)) {
                    for v in it.flatten() {
                        cols.push(v);
                    }
                }
            }
            if cols.is_empty() {
                continue;
            }
            let sel = cols
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(",");
            let mut rows: Vec<Vec<Cell>> = Vec::new();
            if let Ok(mut s) = db.prepare(&format!("SELECT {sel} FROM \"{name}\"")) {
                let rs = s
                    .query_map([], |r| {
                        let mut row = Vec::with_capacity(cols.len());
                        for idx in 0..cols.len() {
                            let v: rusqlite::types::Value =
                                r.get(idx).unwrap_or(rusqlite::types::Value::Null);
                            row.push(Cell::from(v));
                        }
                        Ok(row)
                    })
                    .map_err(|e| e.to_string())?;
                for r in rs {
                    rows.push(r.map_err(|e| e.to_string())?);
                }
            }
            ex.mirrors.push(MirrorTable {
                name: format!("{}__{}", sanitize(&stem), sanitize(&name)),
                columns: cols,
                rows,
            });
        }
        return Ok(Some(ex));
    }

    let (buf, _compressed) = if cprnav::is_cprnav(&data) {
        match cprnav::decompress(&data) {
            Ok(u) => (u, true),
            Err(e) => return Err(format!("decompress: {e}")),
        }
    } else {
        (data.clone(), false)
    };
    if buf.len() < 0x77 {
        return Ok(None);
    }
    let list_id = Some(u16le(&buf, 0x02));
    let file_kind = u32le(&buf, 0x0c);

    if file_kind == 6 && buf.len() >= 0x77 + 36 {
        return match lid_format::rel::RelIndex::parse(&buf) {
            Ok(idx) => {
                let pairs = lid_format::rel::get_relations(&buf, &idx, true, 0, idx.d[2])
                    .unwrap_or_default();
                let mut ex = mk("rel");
                ex.list_id = list_id;
                ex.rel = Some(RelOut {
                    d0: idx.d[0] as u16,
                    d1: idx.d[1] as u16,
                    pairs,
                });
                Ok(Some(ex))
            }
            Err(e) => Err(format!("rel parse: {e}")),
        };
    }
    if lid_format::is_name_list(&buf) {
        let nl = lid_format::read(&buf).map_err(|e| format!("namelist: {e}"))?;
        let mut ex = mk("namelist");
        ex.list_id = list_id;
        ex.block_count = Some(nl.block_count);
        ex.origin = nl.origin;
        ex.coords_valid_override = match coords_mode {
            "file-origin" => Some(true),
            "none" => Some(false),
            _ => None,
        };
        ex.elements = nl
            .elements
            .iter()
            .enumerate()
            .map(|(i, e)| ExportElement {
                elem: i as u32,
                block: e.block,
                name: e.name.clone(),
                sort_name: e.sort_name.clone(),
                category: e.category,
                has_pos: e.has_pos,
                x_pau: e.x_pau,
                y_pau: e.y_pau,
                belonging: e.belonging,
            })
            .collect();
        return Ok(Some(ex));
    }
    if lid_format::is_gen_attr(&buf) {
        let ga = lid_format::read_gen_attr(&buf).map_err(|e| format!("gen_attr: {e}"))?;
        let mut ex = mk("gen_attr");
        ex.list_id = list_id;
        for bi in 0..ga.blocks.len() {
            let blk = match ga.decode_block(&buf, bi) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let stream = |cc: u32, fl: u32| -> Vec<u32> {
                blk.streams
                    .iter()
                    .find(|x| x.col == cc && x.flags == fl)
                    .map(|x| x.values.clone())
                    .unwrap_or_default()
            };
            let to_street = stream(0x002, 0x8000);
            let number = stream(0x00c, 0x8000);
            if to_street.is_empty() && number.is_empty() {
                continue;
            }
            let n = (blk.elem_end - blk.elem_start + 1) as usize;
            for k in 0..n {
                let a = to_street.get(k).copied();
                let h = number.get(k).copied();
                if a.is_none() && h.is_none() {
                    continue;
                }
                ex.hnr.push(sqlite_export::HnrRow {
                    elem: blk.elem_start + k as u32,
                    addr_to_street: a,
                    house_number: h,
                });
            }
        }
        return Ok(Some(ex));
    }
    Ok(None)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn collect(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let rd = match fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("readdir {}: {e}", dir.display());
            return;
        }
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if recursive {
                collect(&p, recursive, out);
            }
        } else if p.is_file() {
            out.push(p);
        }
    }
}

fn dump_file(
    path: &Path,
    filters: &[String],
    no_strings: bool,
    exact: bool,
) -> Result<Value, String> {
    let data = fs::read(path).map_err(|e| format!("read: {e}"))?;
    let base = json!({"path": path.display().to_string(), "size": data.len()});
    let mut obj = match base.as_object().cloned() {
        Some(m) => m,
        None => Map::new(),
    };

    if data.starts_with(b"SQLite format 3") {
        dump_sqlite(path, &mut obj)?;
    } else {
        dump_binary(&data, &mut obj, filters, no_strings, exact);
    }
    Ok(Value::Object(obj))
}

/// Dump every user table of a SQLite file as {table: {columns:[...], rows:[{...}]}}.
fn dump_sqlite(path: &Path, obj: &mut Map<String, Value>) -> Result<(), String> {
    let db = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("sqlite open: {e}"))?;
    let mut stmt = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .map_err(|e| format!("sqlite prep: {e}"))?;
    let tables: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    drop(stmt);

    let mut tmap = Map::new();
    for name in tables {
        // FTS3 shadow tables are noise; keep the virtual/main table, skip *_content/_segments/...
        if name.contains("_content")
            || name.contains("_segments")
            || name.contains("_segdir")
            || name.contains("_docsize")
            || name.contains("_stat")
        {
            continue;
        }
        let mut cols: Vec<String> = Vec::new();
        if let Ok(mut s) = db.prepare(&format!("PRAGMA table_info(\"{name}\")")) {
            if let Ok(it) = s.query_map([], |r| r.get::<_, String>(1)) {
                for v in it.flatten() {
                    cols.push(v);
                }
            }
        }
        if cols.is_empty() {
            continue;
        }
        let sel = cols
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(",");
        let q = format!("SELECT {sel} FROM \"{name}\"");
        let rows = match db.prepare(&q).and_then(|mut s| {
            let mut rows = Vec::new();
            let rs = s.query_map([], |r| {
                let mut row = Map::new();
                for (idx, c) in cols.iter().enumerate() {
                    let v: rusqlite::types::Value =
                        r.get(idx).unwrap_or(rusqlite::types::Value::Null);
                    row.insert(c.clone(), to_json(v));
                }
                Ok(row)
            })?;
            for r in rs {
                rows.push(r?);
            }
            Ok(rows)
        }) {
            Ok(r) => r,
            Err(e) => {
                tmap.insert(name, json!({"columns": cols, "error": e.to_string()}));
                continue;
            }
        };
        tmap.insert(
            name,
            json!({"columns": cols, "row_count": rows.len(), "rows": rows}),
        );
    }
    obj.insert("kind".into(), json!("sqlite"));
    obj.insert("tables".into(), Value::Object(tmap));
    Ok(())
}

fn to_json(v: rusqlite::types::Value) -> Value {
    use rusqlite::types::Value as RV;
    match v {
        RV::Null => Value::Null,
        RV::Integer(i) => json!(i),
        RV::Real(f) => json!(f),
        RV::Text(s) => json!(s),
        RV::Blob(b) => json!({"_blob_len": b.len()}),
    }
}

/// Binary LID/REL/PA file: CPRNAV-decompress if needed, canonical 0x77-header identity,
/// then name-list / GenAttr / REL decode via `lid_format`.
fn dump_binary(
    data: &[u8],
    obj: &mut Map<String, Value>,
    filters: &[String],
    no_strings: bool,
    exact: bool,
) {
    let (buf, compressed) = if cprnav::is_cprnav(data) {
        match cprnav::decompress(data) {
            Ok(u) => (u, true),
            Err(e) => {
                obj.insert("kind".into(), json!("cprnav"));
                obj.insert("error".into(), json!(format!("decompress: {e}")));
                return;
            }
        }
    } else {
        (data.to_vec(), false)
    };
    obj.insert("kind".into(), json!("asf"));
    obj.insert("cprnav_compressed".into(), json!(compressed));
    obj.insert("uncompressed_size".into(), json!(buf.len()));
    // Canonical outer header (§11.2): device binds list files by the leading rIdxListID.
    if buf.len() >= 0x77 {
        obj.insert("header".into(), json!({
            "region_ident": u16le(&buf, 0x00),
            "list_id": u16le(&buf, 0x02),
            "file_kind": u32le(&buf, 0x0c),
            "split": u32le(&buf, 0x10),
            "sub_header_size": u32le(&buf, 0x14),
            "author_block_canonical": buf[0x18..0x77] == lid_format::header::NL_AUTHOR_BLOCK[..],
        }));
    }
    let name_keep = |n: &str| {
        filters.is_empty() || {
            let ln = n.to_lowercase();
            if exact {
                filters.iter().any(|f| ln == f.as_str())
            } else {
                filters.iter().any(|f| ln.contains(f.as_str()))
            }
        }
    };
    // REL relation matrix (file kind 6): full (src, tgt) pair list.
    if u32le(&buf, 0x0c) == 6 && buf.len() >= 0x77 + 36 {
        match lid_format::rel::RelIndex::parse(&buf) {
            Ok(idx) => {
                let out = lid_format::rel::get_relations(&buf, &idx, true, 0, idx.d[2])
                    .unwrap_or_default();
                obj.insert(
                    "rel".into(),
                    json!({
                        "list_rows": idx.d[0], "list_cols": idx.d[1],
                        "src_elems": idx.d[2], "tgt_elems": idx.d[3],
                        "band_stride": [idx.d[4], idx.d[5]], "grid": [idx.d[6], idx.d[7]],
                        "pair_count": out.len(),
                        "pairs": out.iter().map(|&(s, t)| json!([s, t])).collect::<Vec<Value>>(),
                    }),
                );
            }
            Err(e) => {
                obj.insert("rel_error".into(), json!(e));
            }
        }
    }
    // If this is an ASF name-list (the LID*.DAT address trie), decode it into a
    // per-element gazetteer (names + PAU position deltas + hierarchy) via lid_format.
    if u32le(&buf, 0x0c) != 6 && lid_format::is_name_list(&buf) {
        match lid_format::read(&buf) {
            Ok(nl) => {
                obj.insert(
                    "namelist".into(),
                    json!({
                        "element_count": nl.element_count,
                        "block_count": nl.block_count,
                        "origin": nl.origin,
                        "filtered": !filters.is_empty(),
                        "elements": nl.elements.iter().enumerate()
                            .filter(|(_, e)| name_keep(&e.name))
                            .map(|(i, e)| json!({
                                "elem": i,
                                "block": e.block,
                                "name": e.name,
                                "sort_name": e.sort_name,
                                "display": e.name != e.sort_name,
                                "x_pau": e.x_pau,
                                "y_pau": e.y_pau,
                                "has_pos": e.has_pos,
                                "belonging": e.belonging,
                            })).collect::<Vec<Value>>(),
                    }),
                );
            }
            Err(e) => {
                obj.insert("namelist_error".into(), json!(e));
            }
        }
    } else if lid_format::is_gen_attr(&buf) {
        obj.insert("kind".into(), json!("gen_attr"));
        match lid_format::read_gen_attr(&buf) {
            Ok(ga) => {
                let mut g = json!({
                    "element_count": ga.element_count,
                    "block_count": ga.blocks.len(),
                    "blocks": ga.blocks.iter().map(|e| json!({
                        "elem_start": e.elem_start,
                        "elem_end": e.elem_end,
                        "block_off": e.block_off,
                    })).collect::<Vec<Value>>(),
                });
                // Opt-in deep decode of block interiors (NLGeneralAttributeBlock descriptors +
                // attribute-vector streams). Enable with LID2DUMP_BLOCKS=<count|all>.
                if let Ok(s) = std::env::var("LID2DUMP_BLOCKS") {
                    let nb = if s == "all" {
                        ga.blocks.len()
                    } else {
                        s.parse().unwrap_or(1)
                    };
                    // Whole-file joined columns (per address element) + per-block stream detail.
                    let mut to_street = Vec::new();
                    let mut number = Vec::new();
                    let mut bd = Vec::new();
                    for bi in 0..nb.min(ga.blocks.len()) {
                        match ga.decode_block(&buf, bi) {
                            Ok(blk) => {
                                let col = |cc: u32, fl: u32| -> Vec<u32> {
                                    blk.streams
                                        .iter()
                                        .find(|x| x.col == cc && x.flags == fl)
                                        .map(|x| x.values.clone())
                                        .unwrap_or_default()
                                };
                                to_street.extend(col(0x002, 0x8000));
                                number.extend(col(0x00c, 0x8000));
                                bd.push(json!({
                                    "block": bi,
                                    "elem_start": blk.elem_start,
                                    "elem_end": blk.elem_end,
                                    "streams": blk.streams.iter().map(|st| json!({
                                        "col": st.col,
                                        "flags": st.flags,
                                        "code": st.code,
                                        "param": st.param,
                                        "decoded": if matches!(st.code, 0x01|0x02|0x03)
                                            { json!({"set_bits_of": st.bits.len(), "set": st.bits.iter().filter(|&&x| x).count()}) }
                                            else { json!(st.values.iter().copied().take(64).collect::<Vec<u32>>() ) },
                                    })).collect::<Vec<Value>>(),
                                }));
                            }
                            Err(e) => bd.push(json!({"block": bi, "error": e})),
                        }
                    }
                    g.as_object_mut()
                        .unwrap()
                        .insert("addr_to_street".into(), json!(to_street));
                    g.as_object_mut()
                        .unwrap()
                        .insert("addr_number".into(), json!(number));
                    if bd.len() <= 200 {
                        g.as_object_mut()
                            .unwrap()
                            .insert("decoded_blocks".into(), Value::Array(bd));
                    }
                }
                obj.insert("gen_attr".into(), g);
            }
            Err(e) => {
                obj.insert("gen_attr_error".into(), json!(e));
            }
        }
    }
    if !no_strings {
        let strings = ascii_strings(&buf, 4);
        obj.insert("n_strings".into(), json!(strings.len()));
        obj.insert("strings".into(), json!(strings));
    }
}

fn u16le(b: &[u8], o: usize) -> u16 {
    if o + 2 <= b.len() {
        u16::from_le_bytes([b[o], b[o + 1]])
    } else {
        0
    }
}
fn u32le(b: &[u8], o: usize) -> u32 {
    if o + 4 <= b.len() {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    } else {
        0
    }
}

/// Extract printable ASCII runs of length >= min (UTF-8 bytes that are ASCII).
fn ascii_strings(b: &[u8], min: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    for &c in b {
        if (0x20..=0x7e).contains(&c) {
            cur.push(c);
        } else {
            if cur.len() >= min {
                let s = String::from_utf8_lossy(&cur).to_string();
                if s.chars().any(|ch| ch.is_alphabetic()) {
                    out.push(s);
                }
            }
            cur.clear();
        }
    }
    if cur.len() >= min {
        let s = String::from_utf8_lossy(&cur).to_string();
        if s.chars().any(|ch| ch.is_alphabetic()) {
            out.push(s);
        }
    }
    out
}
