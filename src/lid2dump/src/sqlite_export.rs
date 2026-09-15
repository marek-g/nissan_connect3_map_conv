// Relational (SQLite) export mode of lid2dump.
//
// Emits ONE .db file with a normalized, *content-preserving* schema:
//   file    1 row per input file (list_id, kind, country, block anchor, coords_valid)
//   element 1 row per LID element (raw `name`/`sort_name`, raw PAU deltas) — never rewritten
//   rel     REL00001-style pair matrices, endpoints resolved to file ids within the run
//   hnr     GenAttr house numbers -> street element; stock layout: `elem` = file-wide record ordinal,
//           street = 0xc01 owner, number = flat value (§11.6b); legacy osm2lid files: elem = addr element
//   <STEM>__<table>  1:1 mirrors of embedded SQLite tables (GLOB_POI, DB_CITY)
//   queries bundled, documented SELECT statements (the user's join layer)
//
// Views (`v_element`, `v_city`, `v_street`, `v_address`, `v_completeness`) carry the joins;
// absolute lat/lon is exposed ONLY where it is honestly computable (`coords_valid` =
// file anchor known and a single block; multi-block stock files keep per-block anchors
// that are not decoded yet, so their lat/lon stay NULL by design — raw deltas always ship).
// No data cleaning happens here (ZIP prefixes etc. stay inside `sort_name`).

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use rusqlite::types::Value as SqliteValue;
use rusqlite::{Connection, ToSql};

/// A scalar cell from a mirrored embedded SQLite table.
#[derive(Debug, Clone)]
pub enum Cell {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

/// A 1:1 mirror of one table found inside an input SQLite file (GLOB_POI.DAT, DB_CITY.DAT).
#[derive(Debug, Clone)]
pub struct MirrorTable {
    pub name: String, // already sanitized `STEM__table`, unique in the db
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

/// Decoded REL matrix; endpoints are element ids in the lists `d0` (src) / `d1` (tgt).
#[derive(Debug, Clone)]
pub struct RelOut {
    pub d0: u16,
    pub d1: u16,
    pub pairs: Vec<(u32, u32)>,
}

/// One GenAttr (LID4nnnn) address row: element index + the writer's street/number streams.
#[derive(Debug, Clone)]
pub struct HnrRow {
    pub elem: u32,
    pub addr_to_street: Option<u32>,
    pub house_number: Option<u32>,
}

/// One LID element, exactly as `lid_format::read` decoded it (no transformations).
#[derive(Debug, Clone)]
pub struct ExportElement {
    pub elem: u32,
    pub block: usize,
    pub name: String,
    pub sort_name: String,
    pub category: u16,
    pub has_pos: bool,
    pub x_pau: i32,
    pub y_pau: i32,
    pub belonging: u32,
}

/// Everything the exporter needs about one decoded input file.
#[derive(Debug, Clone)]
pub struct ExportFile {
    pub path: String,
    pub name: String, // suggested label/unique prefix (deduped inside the writer)
    pub kind: String, // 'namelist' | 'gen_attr' | 'rel' | 'sqlite'
    pub list_id: Option<u16>,
    pub country: Option<String>,
    pub block_count: Option<usize>,
    pub origin: Option<(i32, i32)>,
    /// None = auto rule (single block + origin); Some(v) overrides the anchor-model decision
    /// (`--coords file-origin` for files written by osm2lid: one global anchor per file).
    pub coords_valid_override: Option<bool>,
    pub elements: Vec<ExportElement>,
    pub rel: Option<RelOut>,
    pub hnr: Vec<HnrRow>,
    pub mirrors: Vec<MirrorTable>,
}

pub const SCHEMA_SQL: &str = r#"
CREATE TABLE file(
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL UNIQUE,          -- display label (stem, deduped)
  path          TEXT,                          -- full input path
  list_id       INTEGER,                       -- canonical header rIdxListID (2=city, 3=street, ...)
  kind          TEXT NOT NULL,                 -- namelist|gen_attr|rel|sqlite
  country       TEXT,
  block_count   INTEGER,
  origin_x      INTEGER,                       -- file tNLHPosition (PAU); NULL = no anchor
  origin_y      INTEGER,
  coords_valid  INTEGER NOT NULL DEFAULT 0,    -- 1 ⇔ origin known AND single block ⇒ abs lat/lon honest
  element_count INTEGER
);
CREATE TABLE element(
  id         INTEGER PRIMARY KEY,
  file_id    INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  elem       INTEGER NOT NULL,                 -- global rank inside NLAsfBlock container
  block      INTEGER,                           -- 0-based NLAsfBlock index
  name       TEXT NOT NULL,                     -- display form (after first TAB), raw
  sort_name  TEXT NOT NULL,                     -- raw device line (incl. TAB lines), raw
  category   INTEGER,                           -- NULL until the per-element category column is decoded
  has_pos    INTEGER NOT NULL DEFAULT 0,
  x_pau      INTEGER, y_pau INTEGER,            -- STORED DELTA vs block/city anchor (raw)
  belonging  INTEGER,                            -- raw 0x40c value; NULL = none (0xffffffff)
  UNIQUE(file_id, elem)
);
CREATE INDEX ix_element_name ON element(name);
CREATE TABLE rel(
  id          INTEGER PRIMARY KEY,
  file_id     INTEGER REFERENCES file(id),
  pair_idx    INTEGER NOT NULL,
  src_file_id INTEGER REFERENCES file(id),      -- resolved in-run by list_id (NULL = not found)
  src_elem    INTEGER NOT NULL,
  tgt_file_id INTEGER REFERENCES file(id),
  tgt_elem    INTEGER NOT NULL
);
CREATE INDEX ix_rel_src ON rel(src_file_id, src_elem);
CREATE INDEX ix_rel_tgt ON rel(tgt_file_id, tgt_elem);
CREATE TABLE hnr(
  file_id        INTEGER NOT NULL REFERENCES file(id),
  elem           INTEGER NOT NULL,
  addr_to_street INTEGER,                       -- street element id (in the list_id=3 file)
  house_number   INTEGER,
  street_file_id INTEGER REFERENCES file(id),   -- resolved in-run (NULL = no street file in run)
  PRIMARY KEY(file_id, elem)
);
CREATE TABLE queries(
  name TEXT PRIMARY KEY,
  note TEXT,                                    -- what it answers; DERIVED = computed, not card data
  sql  TEXT NOT NULL
);
"#;

pub const VIEWS_SQL: &str = r#"
CREATE VIEW v_element AS
SELECT e.id            AS id,
       f.id            AS file_id,
       f.name          AS file,
       f.list_id       AS list_id,
       f.kind          AS kind,
       f.country       AS country,
       e.elem          AS elem,
       e.block         AS block,
       e.name          AS name,
       e.sort_name     AS sort_name,
       e.category      AS category,
       e.has_pos       AS has_pos,
       e.x_pau         AS x_pau,
       e.y_pau         AS y_pau,
       e.belonging     AS belonging,
       CASE WHEN f.coords_valid = 1 AND e.has_pos = 1
            THEN (f.origin_x + e.x_pau) * 180.0 / 2147483648.0 END AS longitude,
       CASE WHEN f.coords_valid = 1 AND e.has_pos = 1
            THEN (f.origin_y + e.y_pau) * 180.0 / 2147483648.0 END AS latitude
FROM element e
JOIN file f ON f.id = e.file_id;

CREATE VIEW v_city AS
SELECT * FROM v_element WHERE list_id = 2;

CREATE VIEW v_street AS
SELECT e.*,
       c.elem AS city_elem,
       c.name AS city_name
FROM v_element e
JOIN rel r    ON r.src_file_id = e.file_id AND r.src_elem = e.elem
JOIN element c ON c.file_id = r.tgt_file_id AND c.elem = r.tgt_elem
WHERE e.list_id = 3;

CREATE VIEW v_address AS
SELECT st.country      AS country,
       st.city_elem    AS city_elem,
       st.city_name    AS city,
       st.elem         AS street_elem,
       st.name         AS street,
       h.elem          AS hnr_elem,
       h.house_number  AS house_number,
       st.longitude    AS longitude,
       st.latitude     AS latitude
FROM hnr h
LEFT JOIN v_street st ON st.file_id = h.street_file_id AND st.elem = h.addr_to_street;

CREATE VIEW v_completeness AS
SELECT f.id AS file_id, f.name AS file, f.kind AS kind, f.list_id AS list_id,
       f.element_count AS elements,
       (SELECT COUNT(*) FROM element e WHERE e.file_id = f.id AND e.has_pos = 0) AS without_pos,
       (SELECT COUNT(*) FROM element e WHERE e.file_id = f.id AND e.category IS NULL) AS without_category,
       f.coords_valid AS coords_valid,
       (SELECT COUNT(*) FROM rel r WHERE r.file_id = f.id
          AND (r.src_file_id IS NULL OR r.tgt_file_id IS NULL)) AS rel_unresolved
FROM file f;
"#;

/// (name, note, sql) — bundled query pack, stored in the `queries` table.
pub const QUERY_PACK: &[(&str, &str, &str)] = &[
    (
        "gazetteer",
        "full address gazetteer; zip_code is intentionally NULL: the LID model carries no ZIP column yet (fill it model-side, not by scraping names)",
        "SELECT country, city, NULL AS zip_code, street, house_number, latitude, longitude
           FROM v_address
          ORDER BY country, city, street, house_number;",
    ),
    (
        "cities_alphabetical",
        "city list in device order (sort_name is the card's own sort key; may start with numeric line-1 prefix)",
        "SELECT country, sort_name, name, latitude, longitude
           FROM v_city
          ORDER BY sort_name;",
    ),
    (
        "streets_in_city",
        "streets of one city; bind :city to a city name from v_city",
        "SELECT DISTINCT sort_name, name, city_elem, elem AS street_elem, latitude, longitude
           FROM v_street
          WHERE city_name = :city
          ORDER BY sort_name;",
    ),
    (
        "addresses_in_city",
        "all house numbers of one city; bind :city",
        "SELECT city, street, house_number, latitude, longitude
           FROM v_address
          WHERE city = :city
          ORDER BY street, house_number;",
    ),
    (
        "line1_prefix_derived",
        "DERIVED ONLY - not card data: leading token of the sort line; unconfirmed whether postal or registry code",
        "SELECT country,
                CASE WHEN instr(sort_name, ' ') > 0
                     THEN substr(sort_name, 1, instr(sort_name, ' ') - 1)
                     ELSE NULL END AS line1_first_token,
                sort_name, name
           FROM v_city
          WHERE sort_name GLOB '[0-9]*'
          ORDER BY sort_name;",
    ),
    (
        "multiline_names",
        "elements whose display differs from the raw device line (TAB multi-line names kept raw: name <> sort_name)",
        "SELECT file, list_id, elem, sort_name, name
           FROM v_element
          WHERE name <> sort_name
          ORDER BY file, list_id, elem;",
    ),
    (
        "missing_data",
        "completeness diagnostics per file (what still needs model work: positions, categories, REL resolution, coords)",
        "SELECT * FROM v_completeness ORDER BY kind, name;",
    ),
    (
        "rel_unmatched",
        "REL pairs whose endpoint list file was not part of the same lid2dump run",
        "SELECT r.pair_idx, r.src_elem, r.tgt_elem, r.src_file_id, r.tgt_file_id
           FROM rel r
          WHERE r.src_file_id IS NULL OR r.tgt_file_id IS NULL
          ORDER BY r.pair_idx;",
    ),
];

/// Export `files` into a fresh SQLite database at `out_path`.
pub fn export(out_path: &Path, files: &[ExportFile]) -> Result<(), String> {
    if out_path.exists() {
        fs::remove_file(out_path).map_err(|e| format!("remove old db: {e}"))?;
    }
    if let Some(p) = out_path.parent() {
        if !p.as_os_str().is_empty() {
            let _ = fs::create_dir_all(p);
        }
    }
    let mut conn = Connection::open(out_path).map_err(|e| format!("open db: {e}"))?;
    write_to(&mut conn, files)?;
    conn.execute_batch("PRAGMA optimize;")
        .map_err(|e| format!("optimize: {e}"))?;
    Ok(())
}

/// Fill an open connection (usable with `:memory:` in tests).
pub fn write_to(conn: &mut Connection, files: &[ExportFile]) -> Result<(), String> {
    conn.execute_batch("PRAGMA foreign_keys=OFF;")
        .map_err(|e| e.to_string())?;
    conn.execute_batch(SCHEMA_SQL)
        .map_err(|e| format!("schema: {e}"))?;

    // unique display names (stems may collide across directories)
    let mut used: HashMap<String, u32> = HashMap::new();
    let mut label_for: Vec<String> = Vec::with_capacity(files.len());
    for f in files {
        let base = if f.name.is_empty() {
            f.path.clone()
        } else {
            f.name.clone()
        };
        let c = used.entry(base.clone()).or_insert(0);
        *c += 1;
        label_for.push(if *c == 1 {
            base
        } else {
            format!("{base}_{}", *c)
        });
    }

    // file ids + in-run list_id -> file id resolution (for rel/hnr endpoints)
    let mut by_list: HashMap<u16, i64> = HashMap::new();
    let mut ids: Vec<i64> = Vec::with_capacity(files.len());
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    for (fi, f) in files.iter().enumerate() {
        let coords_valid = i64::from(match f.coords_valid_override {
            Some(v) => v && f.origin.is_some() && f.kind == "namelist",
            None => f.kind == "namelist" && f.origin.is_some() && f.block_count == Some(1),
        });
        tx.prepare_cached(
            "INSERT INTO file(name,path,list_id,kind,country,block_count,origin_x,origin_y,coords_valid,element_count)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        )
        .map_err(|e| e.to_string())?
        .execute(rusqlite::params![
            label_for[fi],
            f.path,
            f.list_id.map(|x| x as i64),
            f.kind,
            f.country,
            f.block_count.map(|x| x as i64),
            f.origin.map(|(x, _)| x as i64),
            f.origin.map(|(_, y)| y as i64),
            coords_valid,
            if f.kind == "namelist" {
                Some(f.elements.len() as i64)
            } else {
                None
            },
        ])
        .map_err(|e| format!("file {}: {e}", f.path))?;
        let rid = tx.last_insert_rowid();
        ids.push(rid);
        if f.kind == "namelist" {
            if let Some(l) = f.list_id {
                by_list.entry(l).or_insert(rid);
            }
        }
    }

    // elements
    for (fi, f) in files.iter().enumerate() {
        if f.elements.is_empty() {
            continue;
        }
        let fid = ids[fi];
        let mut stmt = tx
            .prepare_cached(
                "INSERT INTO element(file_id,elem,block,name,sort_name,category,has_pos,x_pau,y_pau,belonging)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            )
            .map_err(|e| e.to_string())?;
        for e in &f.elements {
            stmt.execute(rusqlite::params![
                fid,
                e.elem as i64,
                e.block as i64,
                e.name,
                e.sort_name,
                if e.category == 0 {
                    None
                } else {
                    Some(e.category as i64)
                },
                i64::from(e.has_pos),
                if e.has_pos {
                    Some(e.x_pau as i64)
                } else {
                    None
                },
                if e.has_pos {
                    Some(e.y_pau as i64)
                } else {
                    None
                },
                if e.belonging == 0xffff_ffff {
                    None
                } else {
                    Some(e.belonging as i64)
                },
            ])
            .map_err(|err| format!("element {}/{}: {err}", f.name, e.elem))?;
        }
    }

    // rel matrices (endpoints resolved in-run by list_id)
    for (fi, f) in files.iter().enumerate() {
        let Some(r) = &f.rel else { continue };
        let fid = ids[fi];
        let src_fid = by_list.get(&r.d0).copied();
        let tgt_fid = by_list.get(&r.d1).copied();
        let mut stmt = tx
            .prepare_cached(
                "INSERT INTO rel(file_id,pair_idx,src_file_id,src_elem,tgt_file_id,tgt_elem)
                 VALUES(?1,?2,?3,?4,?5,?6)",
            )
            .map_err(|e| e.to_string())?;
        for (i, &(s, t)) in r.pairs.iter().enumerate() {
            stmt.execute(rusqlite::params![
                fid, i as i64, src_fid, s as i64, tgt_fid, t as i64
            ])
            .map_err(|err| format!("rel pair {i}: {err}"))?;
        }
    }

    // hnr streams (street-file resolution needs the namelist map)
    let street_fid = by_list.get(&3u16).copied();
    for (fi, f) in files.iter().enumerate() {
        if f.hnr.is_empty() {
            continue;
        }
        let fid = ids[fi];
        let mut stmt = tx
            .prepare_cached(
                "INSERT INTO hnr(file_id,elem,addr_to_street,house_number,street_file_id)
                 VALUES(?1,?2,?3,?4,?5)",
            )
            .map_err(|e| e.to_string())?;
        for h in &f.hnr {
            stmt.execute(rusqlite::params![
                fid,
                h.elem as i64,
                h.addr_to_street.map(|x| x as i64),
                h.house_number.map(|x| x as i64),
                if h.addr_to_street.is_some() {
                    street_fid
                } else {
                    None
                },
            ])
            .map_err(|err| format!("hnr {}: {err}", h.elem))?;
        }
    }

    // mirrored embedded-SQLite tables (GLOB_POI etc.), raw 1:1
    {
        let mut tnames: HashMap<String, u32> = HashMap::new();
        for f in files.iter().filter(|x| x.kind == "sqlite") {
            for m in &f.mirrors {
                let mut tname = m.name.clone();
                loop {
                    let c = tnames.entry(tname.clone()).or_insert(0);
                    *c += 1;
                    if *c == 1 {
                        break;
                    }
                    tname = format!("{}_{}", m.name, *c);
                }
                let cols_decl = m
                    .columns
                    .iter()
                    .map(|c| format!("\"{c}\""))
                    .collect::<Vec<_>>()
                    .join(",");
                let placeholders = vec!["?"; m.columns.len()].join(",");
                tx.execute(&format!("CREATE TABLE \"{tname}\"({cols_decl});"), [])
                    .map_err(|e| format!("mirror {tname} create: {e}"))?;
                let mut stmt = tx
                    .prepare_cached(&format!(
                        "INSERT INTO \"{tname}\"({cols_decl}) VALUES({placeholders});"
                    ))
                    .map_err(|e| format!("mirror {tname} prep: {e}"))?;
                for row in &m.rows {
                    let vals: Vec<&dyn ToSql> = row
                        .iter()
                        .map(|v| match v {
                            Cell::Null => &rusqlite::types::Null as &dyn ToSql,
                            Cell::Int(i) => i as &dyn ToSql,
                            Cell::Real(r) => r as &dyn ToSql,
                            Cell::Text(s) => s as &dyn ToSql,
                            Cell::Blob(b) => b as &dyn ToSql,
                        })
                        .collect();
                    stmt.execute(vals.as_slice())
                        .map_err(|err| format!("mirror {tname} row: {err}"))?;
                }
            }
        }
    }

    tx.execute_batch(VIEWS_SQL)
        .map_err(|e| format!("views: {e}"))?;
    {
        let mut stmt = tx
            .prepare_cached("INSERT INTO queries(name,note,sql) VALUES(?1,?2,?3)")
            .map_err(|e| e.to_string())?;
        for (n, note, sql) in QUERY_PACK {
            stmt.execute(rusqlite::params![n, note, sql])
                .map_err(|e| e.to_string())?;
        }
    }
    tx.execute_batch("PRAGMA user_version=1; ANALYZE;")
        .map_err(|e| e.to_string())?;
    tx.commit().map_err(|e| e.to_string())?;
    Ok(())
}

impl From<SqliteValue> for Cell {
    fn from(v: SqliteValue) -> Cell {
        match v {
            SqliteValue::Null => Cell::Null,
            SqliteValue::Integer(i) => Cell::Int(i),
            SqliteValue::Real(r) => Cell::Real(r),
            SqliteValue::Text(s) => Cell::Text(s),
            SqliteValue::Blob(b) => Cell::Blob(b),
        }
    }
}
