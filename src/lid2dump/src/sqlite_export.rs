// Relational (SQLite) export mode of lid2dump.
//
// Emits ONE .db file with a normalized, *content-preserving* schema:
//   file    1 row per input file (list_id, kind, country, block anchor, coords_valid)
//   element 1 row per LID element (raw `name`/`sort_name`, raw PAU deltas) — never rewritten
//   rel     REL00001-style pair matrices, endpoints resolved to file ids within the run
//   hnr     GenAttr house-number records: street element + [0xc01..0xc02] range + 0xc09/0xc0a
//           parity bits; the device expands each record n = from, from+step..to with
//           step = (even == odd) ? 1 : 2 (`NLHnrToString::bProcessHnr` 00ce7b80)
//   <STEM>__<table>  1:1 mirrors of embedded SQLite tables (GLOB_POI, DB_CITY)
//   queries bundled, documented SELECT statements (the user's join layer)
//
// Derived (non-raw, but reversible) tables: `addr` splits address-list (list_id 129) display
// names 'CITY, STREET NUM' (the device's own source of a city's street list — it re-derives
// street indices from HNR names, see LISA_tclHnrProcessing::bSetUpStreetIndcesByHnr); `region`
// holds the province list (list_id 9, LID20005 'WOJ. X' + foreign variants) with a derived
// adjective key; `city_prefixed` splits 'ADJ, CITY' city entries (same-name cities in different
// provinces - the car's "which province?" disambiguation) and links them to `region`.
//
// Views (`v_element`, `v_city`, `v_street`, `v_address`, `v_hnr_offered`, `v_city_street`,
// `v_completeness`) carry the joins; `v_hnr_offered` expands every record with the device rule
// absolute lat/lon is exposed ONLY where it is honestly computable (`coords_valid` =
// file anchor known and a single block; multi-block stock files keep per-block anchors
// that are not decoded yet, so their lat/lon stay NULL by design — raw deltas always ship).
// No data cleaning happens here (ZIP prefixes etc. stay inside `sort_name`).

use std::collections::{HashMap, HashSet};
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

/// One GenAttr (LID4nnnn) house-number record (`enGetHnr` 00e0d078): number range [from..to] with
/// parity flags; the device (`NLHnrToString::bProcessHnr` 00ce7b80) expands it to
/// `from, from+step, ... <= to` where `step = (even == odd) ? 1 : 2`.
#[derive(Debug, Clone)]
pub struct HnrRow {
    pub elem: u32,
    pub addr_to_street: Option<u32>,
    pub house_number: Option<u32>,
    /// `0xc02` upper bound; on stock single-number records it equals `house_number`.
    pub house_number_to: Option<u32>,
    /// `0xc09` hasEvenParity (NLHnrStatus::bHasEvenParity).
    pub even: Option<bool>,
    /// `0xc0a` hasOddParity (NLHnrStatus::bHasOddParity).
    pub odd: Option<bool>,
    /// `0xc0b/0xc0c/0xc0d`: stock mirrors the parity bits in `0xc0b/0xc0c` and keeps a
    /// direction-class flag in `0xc0d` ([OPEN] exact semantics; ~half of stock records set).
    pub side: Option<(bool, bool, bool)>,
    /// `0x001` per-entry id (meaning [OPEN]; device joins via `0xc11` -> `block_cells`).
    /// Exported as `hnr.cell`.
    pub cell: Option<u32>,
}

/// One decoded `NLCellIdAttrVector` table row of a GenAttr block (§11.6c: `0x002` NAV-file local
/// ids, `0x004` global/cell ids, `0x005` existence bits, `0x003` per-row description element).
#[derive(Debug, Clone)]
pub struct BlockCellRow {
    pub block: usize,
    pub cell_ord: u32,
    pub elem: Option<u32>,
    pub local_id: Option<u32>,
    pub global_id: Option<u32>,
    pub bit_set: bool,
}

/// One value of a keyed GenAttr cell column (`0xc11` per-record row ordinals, `0x003`/`0xc12`/
/// `0xc14` owner-keyed side lists), flattened: owner element × key ordinal × value slot.
#[derive(Debug, Clone)]
pub struct CellmapRow {
    pub block: usize,
    pub owner_elem: Option<u32>,
    pub key_idx: u32,
    pub col: u32,
    pub k: u32,
    pub value: Option<u32>,
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
    pub block_cells: Vec<BlockCellRow>,
    pub cellmap: Vec<CellmapRow>,
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
  file_id          INTEGER NOT NULL REFERENCES file(id),
  elem             INTEGER NOT NULL,
  addr_to_street   INTEGER,                         -- street element id (in the list_id=3 file)
  house_number     INTEGER,                         -- 0xc01 lower bound (from)
  house_number_to  INTEGER,                         -- 0xc02 upper bound (to); legacy osm2lid files: NULL
  hn_even          INTEGER,                         -- 0xc09 hasEvenParity; device step = 2 when it differs from odd
  hn_odd           INTEGER,                         -- 0xc0a hasOddParity
  hn_c0b           INTEGER,                         -- 0xc0b bit at the record (cell-path status 0)
  hn_c0c           INTEGER,                         -- 0xc0c bit at the record (cell-path status 1)
  hn_c0d           INTEGER,                         -- 0xc0d bit at the record (cell-path status 2)
  cell             INTEGER,                         -- record's cell id (0x001 vector; table in block_cells)
  street_file_id   INTEGER REFERENCES file(id),     -- resolved in-run (NULL = no street file in run)
  PRIMARY KEY(file_id, elem)
);
CREATE TABLE block_cells(
  file_id    INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  block      INTEGER NOT NULL,                    -- GenAttr block index (cell ordinals restart per block)
  cell_ord   INTEGER NOT NULL,                    -- ordinal in the block cell table (= device PA cell id space)
  elem       INTEGER,                             -- 0x003 per-row description element id
  local_id   INTEGER,                             -- 0x002 local cell id (u16 member @+0x530)
  global_id  INTEGER,                             -- 0x004 global cell id (member @+0x80)
  bit_set    INTEGER NOT NULL,                    -- 0x005 existence bit
  PRIMARY KEY(file_id, block, cell_ord)
);
CREATE TABLE cellmap(
  file_id    INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  block      INTEGER NOT NULL,
  owner_elem INTEGER,                             -- owning street elem (record-tiling streams; NULL = raw key)
  key_idx    INTEGER NOT NULL,                    -- key ordinal in the stream's key domain
  col        INTEGER NOT NULL,                    -- 0xc11 / 0xc12 / 0x003
  k          INTEGER NOT NULL,                    -- value slot inside the key's list
  value      INTEGER NOT NULL,
  PRIMARY KEY(file_id, block, col, key_idx, k)
);
CREATE TABLE addr(
  id           INTEGER PRIMARY KEY,
  file_id      INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  elem         INTEGER NOT NULL,                -- element of the list_id=129 name list
  city         TEXT NOT NULL,                   -- DERIVED: prefix matched against the list_id=2 names
  street       TEXT,                            -- DERIVED: remainder without a trailing number token
  house_number TEXT,                            -- DERIVED: trailing token iff it starts with a digit (kept raw, e.g. '3', '12A/2')
  name         TEXT NOT NULL,                   -- original display name, raw
  UNIQUE(file_id, elem)
);
CREATE INDEX ix_addr_city ON addr(city);
CREATE TABLE region(
  id         INTEGER PRIMARY KEY,
  file_id    INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  elem       INTEGER NOT NULL,                  -- element of the province list (list_id 9)
  name       TEXT NOT NULL,                     -- raw ('WOJ. MAZOWIECKIE', 'WOIWODSCHAFT MASOWIEN', ...)
  adjective  TEXT,                              -- DERIVED: 'MAZOWIECKI' from 'WOJ. MAZOWIECKIE' (NULL for foreign forms)
  adjective_ascii TEXT,                         -- DERIVED: ASCII form of the adjective
  UNIQUE(file_id, elem)
);
CREATE TABLE city_prefixed(
  file_id     INTEGER NOT NULL REFERENCES file(id) ON DELETE CASCADE,
  elem        INTEGER NOT NULL,                 -- city element (list_id 2)
  adjective   TEXT NOT NULL,                    -- DERIVED: 'ADJ' of 'ADJ, CITY' (only when it names a province)
  base_name   TEXT NOT NULL,                    -- DERIVED: city name without the province prefix
  base_ascii  TEXT,                             -- DERIVED: adjective/CSV check helper: base name in ASCII
  region_file_id INTEGER,                       -- region.file_id of the matched province (NULL = adjective matched none)
  region_elem    INTEGER,
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
       h.house_number_to AS house_number_to,
       h.hn_even       AS hn_even,
       h.hn_odd        AS hn_odd,
       st.longitude    AS longitude,
       st.latitude     AS latitude
FROM hnr h
LEFT JOIN v_street st ON st.file_id = h.street_file_id AND st.elem = h.addr_to_street;

-- exactly what the car offers for one street when the user types digits: every record's
-- range expanded the device way (NLHnrToString::bProcessHnr): step = (even == odd) ? 1 : 2.
CREATE VIEW v_hnr_offered AS
WITH RECURSIVE gen(file_id, hnr_elem, street_elem, city, street, n, top, st) AS (
  SELECT h.file_id, h.elem, st2.elem, st2.city_name, st2.name,
         h.house_number,
         COALESCE(h.house_number_to, h.house_number),
         CASE WHEN COALESCE(h.hn_even, 0) = COALESCE(h.hn_odd, 0) THEN 1 ELSE 2 END
    FROM hnr h
    JOIN v_street st2 ON st2.file_id = h.street_file_id AND st2.elem = h.addr_to_street
   WHERE h.house_number IS NOT NULL
  UNION ALL
  SELECT file_id, hnr_elem, street_elem, city, street, n + st, top, st
    FROM gen
   WHERE n < top
)
SELECT DISTINCT file_id, hnr_elem, street_elem, city, street, n AS house_number
  FROM gen;

CREATE VIEW v_city_prefixed AS
SELECT p.file_id     AS file_id,
       p.elem        AS city_elem,
       c.name        AS city_name,
       p.adjective   AS region_adjective,
       p.base_name   AS base_name,
       r.name        AS region
FROM city_prefixed p
JOIN element c  ON c.file_id = p.file_id AND c.elem = p.elem
LEFT JOIN region r ON r.file_id = p.region_file_id AND r.elem = p.region_elem;

-- city's streets the way the car browses them: REL00001 edges PLUS the names the device
-- re-derives from the address list (LID20000); `source` says which.
CREATE VIEW v_city_street AS
SELECT DISTINCT 'rel' AS source, city_elem, city_name, name, sort_name, elem AS street_elem
FROM v_street
UNION
SELECT DISTINCT 'addr', e.elem, a.city, a.street, NULL, NULL
FROM addr a
JOIN element e ON e.name = a.city
JOIN file f    ON f.id = e.file_id AND f.list_id = 2 AND f.kind = 'namelist'
WHERE a.street IS NOT NULL;

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
        "streets_in_city_car_view",
        "DERIVED: what the car lists as streets of a city - REL00001 edges ('rel') plus names the device derives from the address list ('addr'); bind :city",
        "SELECT source, city_elem, street_elem, name FROM v_city_street
          WHERE city_name = :city
          ORDER BY name;",
    ),
    (
        "addresses_in_city_list",
        "address-list (LID20000) entries of one city, parsed; bind :city to a city name from v_city",
        "SELECT city, street, house_number, name FROM addr
          WHERE city = :city
          ORDER BY street, house_number;",
    ),
    (
        "provinces",
        "province list (LID20005): all language variants; adjective is the DERIVED key used to match 'ADJ, CITY' city names",
        "SELECT r.elem AS region_elem, r.adjective, r.adjective_ascii, r.name
           FROM region r
          ORDER BY r.name;",
    ),
    (
        "cities_with_province_prefix",
        "DERIVED: city entries spelled 'ADJECTIVE, CITY' (same-name cities in different provinces - the disambiguation the car asks about)",
        "SELECT city_elem, city_name, region_adjective, region FROM v_city_prefixed
          ORDER BY base_name;",
    ),
    (
        "city_regions_rel",
        "raw REL edges between the city list and the province list (e.g. REL00006); both endpoint roles possible",
        "SELECT r.file_id AS rel_file, f.name AS rel_name,
                s.elem AS city_elem, s.name AS city_name,
                t.elem AS region_elem, t.name AS region_name
           FROM rel r
           JOIN file f    ON f.id = r.file_id
           JOIN element s ON s.file_id = r.src_file_id AND s.elem = r.src_elem
           JOIN element t ON t.file_id = r.tgt_file_id AND t.elem = r.tgt_elem
           JOIN file sf ON sf.id = r.src_file_id
           JOIN file tf ON tf.id = r.tgt_file_id
          WHERE (sf.list_id = 2 AND tf.list_id = 9) OR (sf.list_id = 9 AND tf.list_id = 2)
          ORDER BY r.file_id, r.pair_idx;",
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

    // derived province / prefixed-city / address tables (see header comment; raw tables untouched)
    {
        let mut provs: Vec<(i64, u32, String, Option<String>, Option<String>, Option<String>)> =
            Vec::new();
        for (fi, f) in files.iter().enumerate() {
            if f.kind == "namelist" && f.list_id == Some(9) {
                for e in &f.elements {
                    let prov = province_adjectives(&e.name);
                    provs.push((ids[fi], e.elem, e.name.clone(), prov.0, prov.1, prov.2));
                }
            }
        }
        let mut adj_of: HashMap<String, (i64, u32)> = HashMap::new();
        for (fid, el, _, neuter, adj, adj_a) in &provs {
            let mut keys: Vec<String> = Vec::new();
            for opt in [neuter, adj, adj_a].into_iter().flatten() {
                keys.push(opt.clone());
                keys.push(ascii_fold(opt));
            }
            for k in keys {
                adj_of.entry(k).or_insert((*fid, *el));
            }
        }

        let mut ps = tx
            .prepare_cached(
                "INSERT INTO region(file_id,elem,name,adjective,adjective_ascii) VALUES(?1,?2,?3,?4,?5)",
            )
            .map_err(|e| e.to_string())?;
        for (fid, el, name, _, adj, adj_a) in &provs {
            ps.execute(rusqlite::params![fid, *el as i64, name, adj, adj_a])
                .map_err(|err| format!("region {name}: {err}"))?;
        }

        let mut pc = tx
            .prepare_cached(
                "INSERT OR REPLACE INTO city_prefixed(file_id,elem,adjective,base_name,base_ascii,region_file_id,region_elem)
                 VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )
            .map_err(|e| e.to_string())?;
        for (fi, f) in files.iter().enumerate() {
            if f.kind != "namelist" || f.list_id != Some(2) {
                continue;
            }
            for e in &f.elements {
                let Some((adj, base)) = e.name.split_once(", ") else { continue };
                let Some(&(rf, re)) = adj_of.get(adj.trim()) else { continue };
                pc.execute(rusqlite::params![
                    ids[fi],
                    e.elem as i64,
                    adj.trim(),
                    base.trim(),
                    ascii_fold(base.trim()),
                    rf,
                    re as i64
                ])
                .map_err(|err| format!("city_prefixed {}: {err}", e.elem))?;
            }
        }

        let city_forms: HashSet<String> = files
            .iter()
            .filter(|f| f.kind == "namelist" && f.list_id == Some(2))
            .flat_map(|f| f.elements.iter())
            .flat_map(|e| {
                std::iter::once(e.name.clone()).chain(
                    e.sort_name
                        .split('\t')
                        .map(|l| l.trim().to_string())
                        .filter(|l| !l.is_empty()),
                )
            })
            .collect();

        let mut pa = tx
            .prepare_cached(
                "INSERT INTO addr(file_id,elem,city,street,house_number,name) VALUES(?1,?2,?3,?4,?5,?6)",
            )
            .map_err(|e| e.to_string())?;
        for (fi, f) in files.iter().enumerate() {
            if f.kind != "namelist" || f.list_id != Some(129) {
                continue;
            }
            for e in &f.elements {
                let Some((city, rest)) = split_city_prefix(&e.name, &city_forms) else {
                    continue;
                };
                let (street, num) = split_trailing_number(rest);
                pa.execute(rusqlite::params![
                    ids[fi],
                    e.elem as i64,
                    city,
                    street,
                    num,
                    e.name
                ])
                .map_err(|err| format!("addr {}: {err}", e.elem))?;
            }
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
                "INSERT INTO hnr(file_id,elem,addr_to_street,house_number,house_number_to,hn_even,hn_odd,hn_c0b,hn_c0c,hn_c0d,cell,street_file_id)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            )
            .map_err(|e| e.to_string())?;
        for h in &f.hnr {
            stmt.execute(rusqlite::params![
                fid,
                h.elem as i64,
                h.addr_to_street.map(|x| x as i64),
                h.house_number.map(|x| x as i64),
                h.house_number_to.map(|x| x as i64),
                h.even.map(|x| x as i64),
                h.odd.map(|x| x as i64),
                h.side.map(|(b, _, _)| b as i64),
                h.side.map(|(_, c, _)| c as i64),
                h.side.map(|(_, _, d)| d as i64),
                h.cell.map(|x| x as i64),
                if h.addr_to_street.is_some() {
                    street_fid
                } else {
                    None
                },
            ])
            .map_err(|err| format!("hnr {}: {err}", h.elem))?;
        }
    }

    // GenAttr cell tables + keyed cell columns (decoded block streams, §11.6c)
    for (fi, f) in files.iter().enumerate() {
        if f.block_cells.is_empty() && f.cellmap.is_empty() {
            continue;
        }
        let fid = ids[fi];
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO block_cells(file_id,block,cell_ord,elem,local_id,global_id,bit_set)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                )
                .map_err(|e| e.to_string())?;
            for c in &f.block_cells {
                stmt.execute(rusqlite::params![
                    fid,
                    c.block as i64,
                    c.cell_ord as i64,
                    c.elem.map(|x| x as i64),
                    c.local_id.map(|x| x as i64),
                    c.global_id.map(|x| x as i64),
                    c.bit_set as i64,
                ])
                .map_err(|err| format!("block_cells {}: {err}", c.cell_ord))?;
            }
        }
        {
            let mut stmt = tx
                .prepare_cached(
                    "INSERT INTO cellmap(file_id,block,owner_elem,key_idx,col,k,value)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)",
                )
                .map_err(|e| e.to_string())?;
            for m in &f.cellmap {
                stmt.execute(rusqlite::params![
                    fid,
                    m.block as i64,
                    m.owner_elem.map(|x| x as i64),
                    m.key_idx as i64,
                    m.col as i64,
                    m.k as i64,
                    m.value.map(|x| x as i64),
                ])
                .map_err(|err| format!("cellmap {:#x}: {err}", m.col))?;
            }
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

/// Strip Polish diacritics so a name matches its ASCII sibling in the same list.
fn ascii_fold(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'Ą' | 'ą' => 'A',
            'Ć' | 'ć' => 'C',
            'Ę' | 'ę' => 'E',
            'Ł' | 'ł' => 'L',
            'Ń' | 'ń' => 'N',
            'Ó' | 'ó' => 'O',
            'Ś' | 'ś' => 'S',
            'Ź' | 'ź' | 'Ż' | 'ż' => 'Z',
            other => other,
        })
        .collect()
}

/// Province entry name -> (neuter form, masculine adjective, ascii adjective).
/// Only `WOJ. X` entries are decodable this way; foreign ('WOIWODSCHAFT ...') rows yield all-None.
fn province_adjectives(name: &str) -> (Option<String>, Option<String>, Option<String>) {
    let up = name.trim().to_uppercase();
    let Some(neuter) = up.strip_prefix("WOJ. ").map(str::trim) else {
        return (None, None, None);
    };
    if neuter.is_empty() {
        return (None, None, None);
    }
    let adj = neuter.strip_suffix('E').unwrap_or(neuter).to_string();
    (Some(neuter.to_string()), Some(adj.clone()), Some(ascii_fold(&adj)))
}

/// Split an address-list display name `CITY, STREET ...` on the longest `CITY` that is a known
/// city-name form (city names may themselves carry a `ADJ, ` province prefix).
fn split_city_prefix<'a>(name: &'a str, city_forms: &HashSet<String>) -> Option<(&'a str, &'a str)> {
    let mut first: Option<(&'a str, &'a str)> = None;
    let mut best: Option<(&'a str, &'a str)> = None;
    for (pos, _) in name.match_indices(", ") {
        let city = name[..pos].trim();
        if city.is_empty() {
            continue;
        }
        let rest = name[pos + 2..].trim();
        if first.is_none() {
            first = Some((city, rest));
        }
        if city_forms.contains(city) && best.map(|(c, _)| city.len() > c.len()).unwrap_or(true) {
            best = Some((city, rest));
        }
    }
    best.or(first)
}

/// Split the street tail `STREET NAME 12A/2` into (street, trailing house-number token).
fn split_trailing_number(rest: &str) -> (Option<String>, Option<String>) {
    if rest.is_empty() {
        return (None, None);
    }
    match rest.rsplit_once(' ') {
        Some((head, num)) if num.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
            let street = head.trim();
            (
                if street.is_empty() {
                    None
                } else {
                    Some(street.to_string())
                },
                Some(num.to_string()),
            )
        }
        _ => (Some(rest.to_string()), None),
    }
}
