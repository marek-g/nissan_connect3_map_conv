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
  churches/temples), `201`(258), `248`(134), `233`(125)… The ~16 category **names** are known from the
  engine's `enAddPoi` overloads (§5.5); the numeric `CAT_ID` → name mapping is not yet pinned.
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

## 5. `LID0nnnn` — per-cluster POI content block  **[DECODED]**

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
multiple regions (up to 51 MB), all matching a corrected Python reference.

### Decoded structure (from the engine's `fm_tcl*` reader/writer, `DAPIAPP.OUT`)

The unpacked bytes are one **content block** serialized by `fm_tclBlockController`. Layout, in order:

**1. Block header — `fm_tclStartBlockAccess` / `fm_tclStartBlock` (0x28 = 40 bytes).** Written by
`enSetBlock(UniqueID, posLL, posUR, …, BlockType)`:

| offset | type   | field                          | notes |
|--------|--------|--------------------------------|-------|
| +0x05  | u8     | `version_major`                | confirmed: `GetVersionMajor()` reads `this[5]` |
| nearby | u8/u16 | `version_minor` / `version_patch` | written by `enSetBlock`; exact bytes not all pinned |
| +0x18  | 24 B   | `bounding_box`                 | `fm_tclBoundingBox` = lower-left + upper-right `fm_tclPositionWGS84` → the block's geographic extent (confirmed: `GetBoundingBox()` returns `this+0x18`) |
| …      | —      | `unique_id`, `dataset_id`(u8), `draw_prio`, `block_type` | present in `enSetBlock`; exact offsets not all pinned |

**2. POI sequences.** Content is grouped into **sequences** — a run of same-category POIs per
(country, POI-type). Each sequence (`fm_tclPOISeqAccess`) wraps a buffer region; each POI is a `fm_tclPOIData`
record stored inline (zero-copy: `vAddPOIData` only appends a pointer, so the on-disk layout == the struct below).

**3. Point POI record — `fm_tclPOIData` (base reservation 0x14; fields through +0x17).** Offsets confirmed by
decompiling the getters **and** validated against data (an ELL file yields real Estonian lon/lat):

```
+0x06 u16   display_scale   -- min zoom level at which the POI is drawn
+0x08        fm_tclPositionWGS84  (12 bytes)
              +0x08 u32  (unmapped; precision/level?)
              +0x0c u32  longitude   -- PAU = deg * 2^31 / 180
              +0x10 u32  latitude    -- PAU
+0x14 u16   text_id         -- index into the text pool (the POI name)
+0x16 u16   dummy           -- flags/padding
```

**Coordinates are PAU** (same convention as `GLOB_POI` / RNW). Example from `ELL/LID00006`: a record at
`+0x6844` → display_scale 0, lon 24.3812°E, lat 57.8743°N, text_id 46.

**4. Text pool.** Null-terminated name strings (a normalized ASCII-folded form plus accented variants),
referenced by the 16-bit `text_id` (`u16GenerateTextId` / `u16GetTextId`).

**5. POI categories.** `enAddPoi` has ~16 attribute overloads — one per category, mapping onto
`GLOB_POI.CAT_ID`: CarBrand, Shopping, Hotel, Restaurant, Fuel, Medical, City, Landmark, Transport,
ServiceArea, Sporting, Entertainment, Business, PublicBuilding, CarRentalBrand, UserSpecificCategory, Sanctuary.

---

## 6. `LID2/3/4/5nnnn` — map-object files  **[DECOMPRESSED; object structs partially mapped]**

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
payloads. These carry **map objects** serialized by the same `fm_tcl*` module as §5: line objects
(`fm_tclLineData`), polygon objects (`fm_tclPolyData`) and point objects — larger records than a point POI
(e.g. `text_id` sits at `+0x3a` for line/poly; `display_scale` at `+6`). Per-object field layouts are only
partially mapped; the §5 point record is the reference. Compressed siblings are unpacked first with
`cprnav_decompress_rs` (§5).

---

## 7. `CONNECT.DAT` / `META*.DAT`  **[PARTIAL]**

- `CONNECT.DAT` (327 KB): raw index; opens `0d 00 | u32 | u32 | …` and repeats `5c 0c` markers — looks like
  a per-tile connectivity / cross-reference table. Not decoded.
- `META0000.DAT` (196 KB) vs `META9999.DAT` (45 KB): metadata range files; `META9999` carries the common
  container header (§4). The `0000`/`9999` naming suggests first/last of a sparse metadata index. Not decoded.

---

## 8. How LID relates to RNW / MAP

- **RNW** = road-network topology (decoded — see `RNW_format.md`).
- **MAP** = rendered base-map geometry / "FastMap" (see `MAP_format.md`).
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
- **LID content-block structure** (§5): block header (`fm_tclStartBlockAccess`, bounding box @+0x18, version
  @+5), POI sequences, the point-POI record (`fm_tclPOIData`: display_scale@+6, PAU lon/lat @+0x0c/+0x10,
  text_id@+0x14), the text pool, and the ~16 POI categories (→ `CAT_ID`).

**Not fully decoded (next steps, in order):**
1. **Exact byte offsets of every header field** (`unique_id`, `dataset_id`, `draw_prio`, `block_type`) — the
   getters exist but several share names across classes; pin via the `fm_tclStartBlockAccess` accessor set.
2. **Line/polygon object record layouts** (`fm_tclLineData` / `fm_tclPolyData`, §6) for the LID2-5 payloads.
3. **Sequence TOC framing** — how a block enumerates its (country, POI-type) sequences and their lengths.
4. The `CONNECT`/`META`/`REL` index structures (§7).

> Practical note for a converter: if your goal is *navigation*, LID is the optional content layer — the
> network still loads and routes via RNW→MAP without it. If you need POI search / landmark rendering, the files
> now unpack cleanly **and** the point-POI records are decoded (§5), so a POI exporter can be built directly.
