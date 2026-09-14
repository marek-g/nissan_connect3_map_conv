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

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut inputs: Vec<String> = Vec::new();
    let mut out: Option<String> = None;
    let mut recursive = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-o" | "--out" => {
                i += 1;
                out = args.get(i).cloned();
            }
            "-r" | "--recursive" => recursive = true,
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
        match dump_file(t) {
            Ok(v) => files.push(v),
            Err(e) => {
                eprintln!("error: {}: {e}", t.display());
                files.push(json!({"path": t.display().to_string(), "error": e}));
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
        \t-o FILE        write JSON here (default stdout)\n\
        \t-r, --recursive  recurse into directories\n\
        \nDecodes GLOB_POI.DAT / DB_CITY.DAT (SQLite) and LID*.DAT / REL / PA (binary).\n\
        \tAccepts a whole .../LID/CCP/<REGION>/ directory."
    );
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

fn dump_file(path: &Path) -> Result<Value, String> {
    let data = fs::read(path).map_err(|e| format!("read: {e}"))?;
    let base = json!({"path": path.display().to_string(), "size": data.len()});
    let mut obj = match base.as_object().cloned() {
        Some(m) => m,
        None => Map::new(),
    };

    if data.starts_with(b"SQLite format 3") {
        dump_sqlite(path, &mut obj)?;
    } else {
        dump_binary(&data, &mut obj);
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
            let mut rs = s.query_map([], |r| {
                let mut row = Map::new();
                for (idx, c) in cols.iter().enumerate() {
                    let v: rusqlite::types::Value = r.get(idx).unwrap_or(rusqlite::types::Value::Null);
                    row.insert(c.clone(), to_json(v));
                }
                Ok(row)
            })?;
            while let Some(r) = rs.next() {
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
        tmap.insert(name, json!({"columns": cols, "row_count": rows.len(), "rows": rows}));
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

/// Binary LID/REL/PA file: CPRNAV-decompress if needed, then ASF header + ASCII strings.
fn dump_binary(data: &[u8], obj: &mut Map<String, Value>) {
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
    if buf.len() >= 0x26 {
        obj.insert("format_tag".into(), json!(u16le(&buf, 0x00)));
        obj.insert("region_id".into(), json!(u16le(&buf, 0x02)));
        obj.insert("block_count".into(), json!(u16le(&buf, 0x04)));
        obj.insert("hdr_size".into(), json!(u32le(&buf, 0x10)));
        obj.insert("extra_size".into(), json!(u32le(&buf, 0x14)));
    }
    // If this is an ASF name-list (the LID*.DAT address trie), decode it into a
    // per-element gazetteer (names + PAU position deltas + hierarchy) via lid_format.
    if lid_format::is_name_list(&buf) {
        match lid_format::read(&buf) {
            Ok(nl) => {
                obj.insert("namelist".into(), json!({
                    "element_count": nl.element_count,
                    "block_count": nl.block_count,
                    "elements": nl.elements.iter().map(|e| json!({
                        "name": e.name,
                        "x_pau": e.x_pau,
                        "y_pau": e.y_pau,
                        "has_pos": e.has_pos,
                        "belonging": e.belonging,
                    })).collect::<Vec<Value>>(),
                }));
            }
            Err(e) => { obj.insert("namelist_error".into(), json!(e)); }
        }
    } else if lid_format::is_gen_attr(&buf) {
        // GenAttr (fileID+20000, house-number attributes): its container is NLGenAttrFile,
        // not NLNameList — report the block/element TOC (per-block element range) rather
        // than pretending it is a name-list. HNR columns are documented, not yet decoded.
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
                // attribute-vector streams). Enable with LID2DUMP_BLOCKS=<count>.
                if let Ok(s) = std::env::var("LID2DUMP_BLOCKS") {
                    let nb: usize = s.parse().unwrap_or(1);
                    let mut bd = Vec::new();
                    for bi in 0..nb.min(ga.blocks.len()) {
                        match ga.decode_block(&buf, bi) {
                            Ok(blk) => bd.push(json!({
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
                            })),
                            Err(e) => bd.push(json!({"block": bi, "error": e})),
                        }
                    }
                    g.as_object_mut().unwrap().insert("decoded_blocks".into(), Value::Array(bd));
                }
                obj.insert("gen_attr".into(), g);
            }
            Err(e) => { obj.insert("gen_attr_error".into(), json!(e)); }
        }
    }
    let strings = ascii_strings(&buf, 4);
    obj.insert("n_strings".into(), json!(strings.len()));
    obj.insert("strings".into(), json!(strings));
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
