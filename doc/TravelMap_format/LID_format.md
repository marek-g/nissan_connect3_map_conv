# LID (POI / Landmark) Database Format — Bosch TravelMap / Nissan LCN2KAI

Reverse-engineering notes for the `LID` category under
`CRYPTNAV/DATA/DATA/LID/CCP/<REGION>/`. This is the **Points-of-Interest / landmark /
map-object** database and the single largest data category on the card.

Status legend: **[DECODED]** structure confirmed from the data · **[PARTIAL]** header/shape seen,
fields not all mapped · **[NOT DECODED]** needs more work (see §9).

---

## 1. Overview

| | |
| --- | --- |
| Location | `CRYPTNAV/DATA/DATA/LID/CCP/<REGION>/` |
| Size (all regions) | **2.1 GB**, 2109 files — the biggest category (RNW is 3.8 GB, MAP 907 MB) |
| Role | Named POIs (cities, towns, landmarks, churches…), map objects, and text/character resources used by the map engine for rendering + POI search |
| Regions | 16 folders: `ACL BNL CHS DEU EAD EEU ELL FRM GBI GRC IBE ISV MLC POL SCA TUR` |

Each region folder holds **five numbered file families** (the leading digit is the data *type*,
the following digits are a tile/cluster index) plus a handful of special files. The family scheme
is identical across every region:

| family | compressed (all regions) | raw (all regions) | role (inferred) |
| --- | --- | --- | --- |
| `LID0nnnn` | **1096 (always)** | 0 | per-cluster POI list (the bulk of named POIs) |
| `LID2nnnn` | 0 | **296 (always)** | map-object data (type A) |
| `LID3nnnn` | 22 | 25 | map-object data (type B, largest) — mixed |
| `LID4nnnn` | 263 | 33 | map-object data (type A′) — mostly compressed |
| `LID5nnnn` | 42 | 5 | map-object data (type B′) — mostly compressed |

> Compression is **per-file**, not purely per-family: `LID0` is always CPRNAV_2, `LID2` is always raw,
> and `LID3/4/5` are mixed. Total compressed files under `DATA/DATA` = **1423** (all LID). The POL folder
> happens to have only `LID0` compressed + a few large raw `LID2/3/4`; other regions compress more families.
> Note the pairing: in every region `#LID2 == #LID4` and `#LID3 == #LID5` — types 2/4 and 3/5 are parallel
> variants of the same two object kinds.

Special (non-numbered) files, one set per region:

| file | size (POL) | format | role |
| --- | --- | --- | --- |
| `GLOB_POI.DAT` | 450 KB | **SQLite (FTS3)** | searchable POI index — **[DECODED]** §3 |
| `CONNECT.DAT` | 327 KB | raw binary | cross-reference / connectivity index — **[PARTIAL]** §7 |
| `META0000.DAT` | 196 KB | raw binary | metadata (first) — **[PARTIAL]** §7 |
| `META9999.DAT` | 45 KB | raw binary | metadata (last) — carries the common container header |
| `MINITILE.DAT` | 1.2 MB | raw + text | mini-tile overview + text/char resources — **[PARTIAL]** §4 |
| `RELnnnnn.DAT` | 6 files, 11 MB | raw binary | relations (large) — **[PARTIAL]** §4 |

---

## 2. Region breakdown (file counts per family)

```
REGION  LID0  LID2  LID3  LID4  LID5   total
ACL      46    20     3    20     3      92
BNL      54    20     3    20     3     100
CHS      62    21     3    21     3     110
DEU     146     7     1     7     1     162
EAD      36    50     8    50     8     152
EEU     111    30     5    30     5     181
ELL      12    19     3    19     3      56
FRM      83    12     2    12     2     111
GBI      96    13     2    13     2     126
GRC      33     7     1     7     1      49
IBE     115    23     4    23     4     169
ISV      80    17     3    17     3     120
MLC       7    13     2    13     2      37
POL      48     7     1     7     1      64
SCA      63    31     5    31     5     135
TUR     104     6     1     6     1     118
```

---

## 3. `GLOB_POI.DAT` — POI search index  **[DECODED]**

A standard **SQLite** database (magic `SQLite format 3`) using an **FTS3** full-text table. It is
the searchable list of named POIs used for city/landmark lookup and autocomplete. It is **per-region**
(different bytes per region, despite the "GLOB" name) — e.g. every POL row carries `REGION_ID = 1026`.

Virtual table (FTS3):

```sql
CREATE VIRTUAL TABLE GLOBAL_POIS USING FTS3 (
    IDX          NUMBER NOT NULL,   -- POI id within the region
    NAMENORM     VARCHAR(2000) NOT NULL,  -- normalized name (FTS token source)
    NAME         VARCHAR(2000),          -- display name (source language)
    LANG_IDX     NUMBER NOT NULL,        -- language index
    ORIGINAL_IDX NUMBER,                 -- original/global POI id
    LONGITUDE    NUMBER NOT NULL,        -- PAU = deg * 2^31 / 180
    LATITUDE     NUMBER NOT NULL,        -- PAU = deg * 2^31 / 180
    CAT_ID       NUMBER NOT NULL,        -- POI category (type code)
    REGION_ID    NUMBER NOT NULL,        -- region id (POL = 1026)
    CONTROL      NUMBER NOT NULL         -- flags/control word
);
```

- **Coordinates are PAU** (`deg · 2³¹/180`), same scale as the RNW header (§RNW 2). E.g. POL POI
  `lon=196754051 → 18.39°E`, `lat=606618374 → 56.74°N`.
- **`CAT_ID`** is a POI category code. POL distribution (top): `18`(832), `241`(740), `224`(315,
  churches/temples), `201`(258), `248`(134), `233`(125)… — the full category table is not yet mapped.
- **`NAME`** is in the source language of the POI (e.g. Hungarian, German names appear even in the
  POL index for cross-border landmarks).
- Row count (POL): **2854**.

> This file is trivially reproducible: it's plain SQLite. A converter can `CREATE VIRTUAL TABLE … FTS3`
> and insert rows; no proprietary encoding involved.

---

## 4. Common raw-file container + text/char resources  **[PARTIAL]**

The raw (uncompressed) files — `RELnnnnn`, `MINITILE`, `META9999`, and the `LID2/3/4/5` family —
share a common header that embeds provenance metadata followed by **named sections**. Observed layout
(bytes, little-endian):

```
+0x00  (varies by file type)
...
     u16  len            -- length of the following copyright string
     "Copyright by Robert Bosch Car Multimedia GmbH\0"   (or "Bosch 2010")
     "07.06.2021\0"      -- build date
     <section key>\0     -- e.g. "TPLID_EQUIVALENT_CHAR"
     <section payload>
```

- **`TPLID_EQUIVALENT_CHAR`** is a named section holding a **character-equivalence table** (a run of
  small u16 code mappings) — used to normalize/alias characters when rendering POI text.
- `MINITILE.DAT` additionally carries free-form **text resources** in the source language, e.g. the
  German `"Was liegt am Strand und hat einen Sprachfehler? Eine Nuschel."` (a joke / POI blurb). So
  MINITILE bundles a low-zoom tile overview **and** localized text strings.

The exact section framing (how keys/payloads are delimited and how many sections per file) is not yet
pinned.

---

## 5. `LID0nnnn` — per-cluster POI data  **[DECOMPRESSED; records not decoded]**

These are the main POI payload files, one per cluster/tile. They are **CPRNAV_2-compressed**:

```
+0x00 u16  version   = 5
+0x02 u16  unknown   = 0x40 (64)   -- block_size = unknown * 0x400
+0x04 "CPRNAV_2"
+0x0c u32  unpacked_size
+0x10 u16  mode      = 3 (compressed)
+0x12 u16  (1)
+0x14 u32  first_block_offset
+0x18 ...   block-offset table (u32 cumulative ends) until first_block_offset, then compressed blocks
```

**Decompression — SOLVED.** A corrected decompressor is in the repo:
`src/cprnav_decompress_rs/` (`cprnav_decompress_rs`), verified byte-exact against the firmware. The
reference tool `Firmware/tools/lcn2kai-decompress/DecompressAlgorithm.py` fails on every LID file because it
hard-codes a 16-bit per-block header; that is the *only* difference from MAP/IDX files:

- **`block_size = unknown × 0x400`.** `unknown=16` (MAP/IDX) → `0x4000`; `unknown=64` (all LID) → `0x10000`.
- **Per-block header width depends on block size** (`cpr_tclDecompressAlgorithm::vInterpreteHeader`):
  when `block_size < 0x10000` the per-block `info_size`/`out_size` are read as two 16-bit WORDs;
  when `block_size >= 0x10000` they are read as two 32-bit DWORDs. The reference tool always used WORDs, so
  for LID it mis-read the header and diverged from bit 0 (the "only literals / over-read" symptom).

Everything else is identical to the reference: same LSB-first bit reader, same four standard code tables
(`cpr_tclCodeTable::vSetStandardTable`), same `COPY_BYTE`/`COPY_BYTES`/`COPY_PREV` loop. Verified byte-exact
on `N1E10AA.IDX` (`unknown=16`, vs the known-good unpacked copy) and across LID families `LID0/3/4/5` in
multiple regions (up to 51 MB), all matching a corrected Python reference. Unpacked `LID0` = per-cluster POI
records; unpacked `LID3/4/5` carry the common container (§4) with named sections such as
`TPLID_EQUIVALENT_CHAR`.

**Still not decoded:** the per-record layout inside the unpacked bytes (read against the `GLOB_POI` schema in
§3 / the engine's LID reader).

---

## 6. `LID2/3/4/5nnnn` — map-object files  **[DECOMPRESSED where compressed; records not decoded]**

`LID2` is always raw; `LID3/4/5` are mixed — some raw, some CPRNAV_2 (`unknown=64`). The compressed ones now
decompress with the §5 tool (`cprnav_decompress_rs`); the raw ones start directly with the opening below.
Common raw opening (POL example, `LID20002.DAT`):

```
+0x00 u16  = 0x0402 (1026)   -- REGION_ID (matches GLOB_POI POL id)
+0x02 u32  = per-file value  -- (0a/02/08/0c/09/03…) — count or type, not yet mapped
+0x06 u16  = 0x41ec (16876)  -- constant across POL raw files
+0x08 ...
```

Sizes vary hugely (KB to 25 MB); the large ones (`LID30006`, `LID40006` ≈ 25 MB) are the heavy map-object
payloads. Record layout not yet decoded — needs the LID reader in the map engine (Ghidra). Compressed
siblings are unpacked first with `cprnav_decompress_rs` (§5).

---

## 7. `CONNECT.DAT` / `META*.DAT`  **[PARTIAL]**

- `CONNECT.DAT` (327 KB): raw index; opens `0d 00 | u32 | u32 | …` and repeats `5c 0c` markers — looks like
  a per-tile connectivity / cross-reference table. Not decoded.
- `META0000.DAT` (196 KB) vs `META9999.DAT` (45 KB): metadata range files; `META9999` carries the common
  container header (§4). The `0000`/`9999` naming suggests first/last of a sparse metadata index. Not decoded.

---

## 8. How LID relates to RNW / MAP

- **RNW** = road-network topology (decoded — see `RNW_format.md`).
- **MAP** = rendered base-map geometry / "FastMap" (see `MAP_IDX_format.md`).
- **LID** = the *content* layer on top: named POIs, map objects, and text. RNW/MAP give the roads and
  tiles; LID gives the searchable landmarks and the objects drawn on them.

---

## 9. Decoded vs not-decoded — summary & next steps

**Decoded / reproducible now:**
- File organization (regions × families + special files).
- `GLOB_POI.DAT` fully (SQLite FTS3 schema, PAU coords, `REGION_ID`, `CAT_ID`) — trivially writable.
- **CPRNAV_2 decompression for both header widths** (`block_size = unknown×0x400`; 16-bit vs 32-bit
  per-block sizes) → `src/cprnav_decompress_rs/` unpacks every compressed LID and MAP/IDX file.
- Common raw-file container shape (copyright/date/named sections) and the `TPLID_EQUIVALENT_CHAR` table.

**Not decoded (next steps, in order):**
1. **Decode the LID record layouts** using the engine's LID reader (Ghidra): `LID0` per-cluster POI records,
   `LID2/3/4/5` object records, and the `CONNECT`/`META`/`REL` index structures — now readable on unpacked bytes.
2. **Map the `CAT_ID` POI category table** (from the engine's POI-type enum).
3. Pin the common-container section framing (key/payload delimiters, multi-section layout).

> Practical note for a converter: if your goal is *navigation*, LID is the optional content layer — the
> network still loads and routes via RNW→MAP without it. If you need POI search / landmark rendering, the
> files now unpack cleanly (step 1 is decoding the records inside).
