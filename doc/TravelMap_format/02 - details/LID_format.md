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

| family | compressed (all regions) | raw (all regions) | role |
| --- | --- | --- | --- |
| `LID0nnnn` | **1096 (always)** | 0 | **Address-search gazetteer** = `fm_tcl` POI sequences grouped by *name-list type* (city/street/…) + country; its text pool holds the place/street strings (`NAME/code/COUNTRY`, `ULICA …`). **Read directly by OSDE** (§10). Also carries landmark POIs. |
| `LID2nnnn` | 0 | **296 (always)** | map-object data (type A) — `fm_tcl` line/poly/point objects |
| `LID3nnnn` | 22 | 25 | map-object data (type B, largest) — mixed |
| `LID4nnnn` | 263 | 33 | map-object data (type A′) — mostly compressed |
| `LID5nnnn` | 42 | 5 | map-object data (type B′) — mostly compressed |

> **Two file openings, two purposes (CONFIRMED from bytes).** `LID0*` (and `CONNECT.DAT`) open with
> `0d 00` (u16 = `0x000d`) = the **LISA container** that OSDE's `NLDataBlockReader` walks; `LID2/3/4/5`
> open with `02 04` (u16 = `0x0402` for POL = **REGION_ID / dataset_id** = regionIdent `0x402`) = the same
> `fm_tcl` content wrapped as a map-object container. In both, `0x0402` is the region id, `0x0a000000` a
> count and `0x41ec` a constant section marker. The exact LISA **container framing** of `LID0` (the part the
> `fm_tcl*` layer does *not* parse — it is fed an already-located block) is the **one byte-level piece still
> to pin for the writer**; `lid2dump` decodes it against stock (§10.6).

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
`src/cprnav_decompress/` (`cprnav_decompress`), verified byte-exact against the firmware. The
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

### 5b. Confirmed `fm_tcl` framing (from accessor bodies, `DAPIAPP.OUT`)

All `fm_tcl*Access` classes are zero-copy views over a raw buffer (`*(base+const_off)`), so on-disk offsets
read straight out of the getters. The non-`Access` structs (`fm_tclPOIData` etc.) carry a 4-byte typebase
prefix, which is why the *in-memory* getters sit **4 bytes higher** than the on-disk fields (e.g. POI
`GetDisplayScale` in-mem `@+6` `00d162b0` vs on-disk `@+2`). For **writing**, use the on-disk numbers.

**Block header — `fm_tclStartBlock`, fixed `0x28` bytes** (writer `enSetBlock` `00d40660`):

| Off | size | field | setter |
| --- | --- | --- | --- |
| +0x00 | u8 | binary_code = `1` | `vSetBinaryCode` `00d17108` |
| +0x01..03 | u8×3 | version_major / _minor / _patch=`1` | `00d17120/38/50` |
| +0x04 | u32 | block_total_size (init 0x28) | `vSetSize` `00d1716c` |
| +0x08,0x0c | u32×2 | unique_id (first/second) | `vSetUniqueID` `00d2295c` |
| +0x10..0x1f | 4×i32 | bbox LL.lon, LL.lat, UR.lon, UR.lat (**PAU**) | `vSetBoundingBox` `00d23d50` |
| +0x20 | u32 | dataset_id (= REGION_ID, POL=`0x402`) | `vSetDatasetID` `00d17184` |
| +0x24 | u8 | draw_prio | `vSetDrawPrio` `00d1719c` |
| +0x25 | u8 | block_type | `vSetBlockType` `00d2512c` |
| +0x26..27 | 2 | padding |

**Sequence header — `fm_tclPOISeq` / `fm_tclLineSeq`, `0x0c` bytes** (then `count` records @ `+0x0c`):

| Off | size | field |
| --- | --- | --- |
| +0x00 | u8 | binary_code |
| +0x01 | u8 | **name-list type** = `fm_tenPOIType` → maps to OSDE category (§10.2: 2=city,3=street,…) |
| +0x02 | u16 | sequence total size |
| +0x04 | u16 | country (ISO code) |
| +0x06 | u16 | dummy |
| +0x08 | u32 | **element_count** (records) — updated by `vAddPOIData` `00d2a97c` |

This `type`+`country` sequence grouping **is** the OSDE "name list": a city list = a `type=2` sequence, a
street list = `type=3`. Line/poly sequences (§6) share the byte-identical header.

**Text pool — `enSetTextSeq` `00d3c38c`:** seq `binary_code=5`; a raw concatenated string blob, then one
length-prefixed `fm_tclTextStructure` per text-id; `u16GenerateTextId` `00d3b53c` assigns ids sequentially,
so `text_id` = nth structure → its `{offset,length}` into the blob. Place/street display strings
(`NAME/code/COUNTRY`, `ULICA …`) live here.

**POI record — `fm_tclPOIData` (`0x14`):** `size@+0, display_scale@+2, lon i32, lat i32, text_id@+0x0c, dummy@+0x0e`.
> ⚠ The two lon/lat on-disk offsets are **unresolved**: the accessor path implies `@+0x04/@+0x08`, but an
> empirical read of a real ELL block matched `lon@+0x0c / lat@+0x10 / text_id@+0x14` (§3/here). `lid2dump`
> must parse a *known* city/street sequence to settle the exact record base before the writer is trusted.

**Line/Poly record (§6):** `size@+0, display_scale@+2, coord-list-bytes u32@+0x04, 0x0e-byte header, then
PositionWGS84 points`; `text_id` exact on-disk offset ≈ `+0x36` (INFERRED; in-mem `@+0x3a`).

---

## 6. `LID2/3/4/5nnnn` — map-object files  **[DECOMPRESSED; object structs partially mapped]**

`LID2` is always raw; `LID3/4/5` are mixed — some raw, some CPRNAV_2 (`unknown=64`). The compressed ones now
decompress with the §5 tool (`cprnav_decompress`); the raw ones start directly with the opening below.
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
`cprnav_decompress` (§5).

---

## 7. `CONNECT.DAT` / `META*.DAT`  **[PARTIAL]**

- `CONNECT.DAT` (327 KB): raw index; opens `0d 00 | u32 | u32 | …` and repeats `5c 0c` markers — looks like
  a per-tile connectivity / cross-reference table. Not decoded.
- `META0000.DAT` (196 KB) vs `META9999.DAT` (45 KB): metadata range files; `META9999` carries the common
  container header (§4). The `0000`/`9999` naming suggests first/last of a sparse metadata index. Not decoded.

---

## 8. How LID relates to RNW / MAP (and why address search needs it)

- **RNW** = road-network topology (decoded — see `RNW_format.md`).
- **MAP** = rendered base-map geometry / "FastMap" (see `MAP_format.md`).
- **LID** = the *content* layer on top: named POIs, map objects, and text. RNW/MAP give the roads and
  tiles; LID gives the searchable landmarks and the objects drawn on them.

> **Address search is served by LID, NOT by RNW/MAP.** The car's "city → street → house number"
> destination lookup is an entirely separate subsystem (**OSDE/LISA**, §10) that queries the LID
> name-lists, a global city SQLite DB, relation files and HNR point files. **A converter that only writes
> MAP + RNW does not update the address database** — the roads will route and draw, but the address field
> will keep using the stock (stale) LID. To refresh addresses you must also emit LID (§10, `osm2lid`).

---

## 10. Address search (OSDE / LISA) — where city / street / house-number data lives  **[MOSTLY DECODED]**

The destination-address lookup runs in the **LISA** name-list subsystem, configured by
`CRYPTNAV/CFG/LID/OSDE/ADDRESS/CONF.XML` (weights `WEIGHT_CITY/CITYDISTRICT/ZIP/STREET/CROSS/HNR`,
`FACTOR_HNR_MATCH/NEARBY/NO_HNR`, `only_search_in_street_list`). It reads the LID file family directly —
**never the road network**. Everything below was traced in `DAPIAPP.OUT`.

### 10.1 File-type → physical file (CONFIRMED: `LISA_tclDataManager::bGetFileNameFromDataAddress` @`00bc4d54`)

| LISA file type | file |
| --- | --- |
| `0x13/0x14/0x15` | `LID%05u.DAT` (`0x15` = fileID+90000) |
| `0x16` | `REL%05u.DAT` (relations: city→street / city→district, per listID) |
| `0x17` | `POSTILES.DAT` |
| `0x18` | `PA_%05u.DAT` — **point addresses = exact house-number coordinates** |
| `0x03` | `DB_CITY.DAT` — **SQLite FTS** global city gazetteer (+ zips) |
| `1/2/0x11/0x12/0x22/0x23/0x32/0x33` | `META%04u.DAT` (PSF meta/ref tables) |
| `0x21/0x31` | `CONNECT.DAT` (root / region connect entries) |
| `0x42/0x52` | `CONF.XML` / `AREA_CODES.XML` under `CFG/LID/OSDE/ADDRESS/` (path @`bDetermneAccessPathAndMediumID` `00bc3784`) |

**FileID bands inside one region's LID family (CONFIRMED):** `+0` = name lists (city/street), `+10000` =
**crossings** (`bGetCrossingFileID` `00bd4188`), `+20000` = **GenAttr / HNR** (`bGetGenAttrFileID` `00bd4080`).

### 10.2 Name-list category ids (CONFIRMED: `bSendListUpdate` `00c74fc8`, tag table `enParseLineForTags` `00c84cfc`)

`2`=CITY · `3`=STREET · `4`=JUNCTION/CROSS · `5`=HOUSENUMBER · `0x3d`(61)=ZIP · `12`=city-district · `9`=state ·
`0x76`(118)=`OSDE_ADDRESSES` pseudo-category. A city/street "name list" is an **ASF block** (§11) carrying the
names as a string column + positions as a position column; the **category** is fixed by which block/
relation the search selects (cat 2 city list, cat 3 street list), *not* by a `type` byte inside a flat POI record.
(The `fm_tcl` `type`-tagged POI sequences of §5/§5b are the **landmark-POI content** blocks; the address
name-lists use the columnar PSF/ASF layout of §11.)

> **Format reality check.** The address name-lists are a **compressed column-store** (PSF/ASF), materially
> harder to emit than the `fm_tcl` blocks: each block is a table of typed, independently-compressed columns
> (codes `0x11`–`0x18`: raw / RLE / bitmap / **LEB128 VLE** / delta-VLE / sparse / **Simple9**), with
> positions in a dedicated `NLPositionAttrVector`. This is why the plan is **reader (`lid2dump`) first**: it
> must decode these columns against stock before any writer can be trusted. See §11.

### 10.3 Category → data source (CONFIRMED unless marked)

| Field | Source | Accessor |
| --- | --- | --- |
| **CITY** | regional: `LID0` name-list cat 2; global: `DB_CITY.DAT` SQLite `GlobalCityList` | `vPopulateCityIndices` `00c73b68`; `bGetGlobalCityList` `00c96384` (`SELECT NameNorm,Longitude,Latitude,Province_ID FROM GlobalCityList … MATCH …`) |
| **CITYDISTRICT** | same city list (district = extra list element), linked by `REL` relation **type 2** | `vAddCityDistrictInCity` `00c7f258` → `NLRelationProcessor::bGetRelationsPerIndexByType(2)` |
| **ZIP** | `DB_CITY.DAT` `GlobalCityList` zips-as-NameNorm; LID zip-aliases (cat `0x3d`) | `bGetProvinceIdFromZIPCode` `00c952f4`; `bGetCityByID` `00c955d8` (returns `CAT_ID`) |
| **STREET** | `LID0` name-list cat 3 (**no separate street table/DB**) | `bVerifyList` `00c75bf4`; `NLPAOSDEStreetIdxCollector` `00ce8abc` |
| **CROSS** | `LID0` with **fileID+10000** (crossing variant), street pairs by index | `poGetNewNLCrossingProcessor` `00bd41b4`; `enGetCrossingStreetIndices` `00e086c4` |
| **HNR** | `LID0` with **fileID+20000** (GenAttr blocks keyed by street element idx); exact coords from `PA_%05u.DAT`; else **interpolated** along the street | `poGetNewNLGenAttrProcessor` `00bd40ac`; `bGetHnrs` `00ce72a4`; `enGetHnr` `00e0d078`; `bGetPACells` `00be072c`; `bGetInterpolationRatio` `00ce4098` |

**HNR record (INFERRED from `enGetHnr`):** `{ u32 hnr; u32 prefixNum; number/addition/prefix/suffix strings;
NLHnrStatus{ even-parity, odd-parity, refuseInterpolation, countedAgainstDigits }; u32 rangeEnd (-1=none) }`.
Position = `PA` point if present, else `pos = street[i + idx/(n-1)]` between the range bounds unless
`refuseInterpolation`. So house numbers are **attribute records + optional exact points, keyed to a street**
— *not* LID polygons and *not* RNW interpolation.

### 10.4 End-to-end flow (typed "city → street → number")

1. `bProcOSDEAddress` `00c337b0` → `bSearchAddress` `00c771a8`.
2. Parse line by tags (`enParseLineForTags` `00c84cfc`: `city:→2 zip:→0x3d street:→3 houseno:→5`).
3. `bSearchRootNames` `00c75f28` locates region roots (`CONNECT.DAT`/META PSF) → `bSearchStreetsAndCities` `00c766b4`
   matches the `LID0` name lists per category (`only_search_in_street_list` = whitelist `{3}`).
4. `LISA_tclHypothesisValidator::bValidate` over `REL%05u` (city→street, city→district); districts via `00c7f258`.
5. `bSendListUpdate` `00c74fc8` builds result descriptors `city:..;street:..;houseno:..`.
6. Selection → `bGetCellsForOSDE` `00b8b900` (`bReadCrossingCells`/`bReadHNRCells`/`bGetPACells`).
7. Resolve coordinate (GenAttr HNR + `PA`, else interpolate; city = LID element WGS84 or `DB_CITY`) →
   handed to the router via `posfi_tclMsgSetPositionByLocation`.

### 10.5 `DB_CITY.DAT` / `GLOB_POI.DAT` (SQLite)

- `DB_CITY.DAT` (optional; **absent on the reviewed EUR card** — regional LID name-lists are used instead):
  table `GlobalCityList(ID, Name, NameNorm, Longitude, Latitude, Province_ID, CAT_ID)`, FTS `MATCH`; **`CAT_ID == 0x3f`(63) = City**; zips are `NameNorm` entries. Path = connect-global folder + `DB_CITY.DAT` (`vGetGlobalCityListFileName` `00bc3cdc`).
- `GLOB_POI.DAT` (§3) is the **POI-search** gazetteer (queried by `bSearchPOIs` `00c1e2ec` / `bGetPOIList` `00c1de98`).
  Its `CAT_ID` = an internal **FI category id**, mapped through `CFG/LID/POI_SRV/POI_MAPPING.DAT` (`IndexCategories`
  + `neh_IDTable`); `REGION_ID` = the region's regionIdent (POL = 1026 = `0x402`); `CONTROL` observed always `1`;
  `LANG_IDX` = per-language index (`NAME` is that language's display form, `NAMENORM` the ASCII-folded FTS source;
  `ORIGINAL_IDX` groups the same POI across languages). **Streets/HNRs have no CAT_ID — they are LID name-list
  constructs, not POI-DB rows.**

### 10.6 Decoded vs not — writer implications (drives `osm2lid`)

**Writable now:** `GLOB_POI.DAT` / `DB_CITY.DAT` SQLite (plain SQLite, exact schema known). `LID0` POI records +
text pool (block/seq/POI record layouts §5).
**Still to pin byte-level (via `lid2dump`, in progress):** the `LID0` **LISA container** framing (`0d00`…), the
**`type`-tagged name-list sequences** for cat 2/3, the `REL`/GenAttr/`PA` file formats, and the `META`/`CONNECT`
PSF connect tables. These four back **street** and **house-number** search; until written, a converter's
address data is POI/city-only. **Road network is not involved** (CONFIRMED — no `rnw_tcl*` callee in the OSDE path).

---

## 11. The address name-list internal format = compressed column store (PSF/ASF)  **[MOSTLY DECODED]**

`LID%05u.DAT` (fileType 0x13) is parsed by `NLNameList::LoadHeader` @`00e0e63c` + `NLProcessor::bInitialise`
@`00cee6e4` + `NLAsfBlock::SetDataBlock` @`00cdf404`. It is a **column store**, not the `fm_tcl` block of §5.

### 11.1 File-id arithmetic within a region (CONFIRMED: `bValidate` `00c83324`)

name-list = `LID%05u` @ `base`; crossing = @ `base+10000`; **GenAttr/HNR = @ `base+20000`**; PA = `PA_%05u`
(fileType 0x18); relation = `REL_%05u` (fileType 0x16). Name-list / crossing / GenAttr share the **same**
ASF container + parser.

### 11.2 File header — 38 bytes, no ASCII magic (CONFIRMED)

`+0x00 u16`=format tag (`13` in these files), `+0x02 u16`=region code, `+0x04 u16`=**block count**
(=the recurring `0d 00` = 13, NOT a binary code), then info u16/u32s; `+0x10 u32`=**hdrSize**, `+0x14 u32`=
**extraSize**; `+0x18..` five u16 **string offsets** → embedded `LID-COPYRIGHT`/`CREATE DATE`/`SOFTWARE`/… ASCII
block (the only plaintext in the file). Two compressed **u32 vectors** follow: block **offsets** (obj `+0x9c`)
and **sizes** (`+0xac`); count in the ASF sub-header `+0x04`.

### 11.3 Block = 8-byte header + column-descriptor TOC + compressed columns (CONFIRMED)

Block header `NLAsfBlock` @`00cdf404`: `u16 total, u16 x, u16 y, u16 ndescr` (leaf count = total−x). Then
`ndescr` × `NLPSFListDescriptor` **12 bytes** each (`enSetListDescriptions` `00cdc0c0`):
`{ u16 tag (tag&0xfff = column kind), u16 code/flags, u32 data_offset, u32 param }`; a column's byte span =
`offset[i+1] − offset[i]` (last = blockEnd−offset). **Column-kind tags:** ASF `0x401…0x416` (0x401 SimpleList
<u16>, 0x403 EdgeLabelList, 0x404/5 ValueList<u16>, 0x406 SimpleList<u32>, **0x407 PositionAttrVector**,
0x408–0x40c/0x40e SingleValue<u32>, 0x40d–0x416 Binary/BinList). Block type = `entry & 0x1007` (`00cacf64`).

### 11.4 Column encodings — `NLStandardDecoder::Decode<T>` (CONFIRMED, code ∈ 0x11–0x18)

| code | scheme | | code | scheme |
|---|---|---|---|---|
| `0x11` | raw LE | | `0x15` | RLE + VLE |
| `0x12` | RLE (value = run len) | | `0x16` | delta-VLE (prefix sum) |
| `0x13` | bitmap (set positions) | | `0x17` | sparse-fill, delta-VLE gaps |
| `0x14` | **VLE = unsigned LEB128** (`00cdb218`) | | `0x18` | **Simple9** (`00cdc3bc`) |

Fixed ints little-endian (`ReadUnsigned<u32>` `00cdb2e8`). Code `0x0d` is **invalid** (decoders reject it) →
confirms the header `0d 00`s are counts, and `0x0dNN` high-bytes elsewhere are PA/GenAttr column tags.

### 11.5 Positions are a COLUMN (CONFIRMED) — resolves the record-offset conflict

A block's places have **no per-record lon/lat field**. Positions live in `NLPositionAttrVector` (tag `0x407`):
a separate compressed **X** column (`+0x20`) + **Y** column (`+0x30`) + a delta/origin `tNLHPosition` (`+0x48`);
element *i*'s coordinate = origin + `(X[i], Y[i])`. (The empirical `lon@+0x0c/lat@+0x10/text_id@+0x14` belongs to
the flat `fm_tcl` **POI** record of §5b — a *different* block type — which is why it did not match the name-list.)

### 11.6 GenAttr/HNR columns (fileID+20000) (CONFIRMED: `enDecodeHnr` `00e0c09c` / `enGetHnr` `00e0d078`)

Column-oriented per street element: owner/descr index col `+0x28`; HNR token + addition strings (ValueList<u8>)
`+0xb0/0x104/0x158/0x1ac`; **even-parity** bit-vector `+0x260`, **odd-parity** `+0x280`, refuse-interpolation
`+0x2a0`, counted-against-digits `+0x300`; **range-end** `+0x320` (`0xffffffff` = none); owner descr `+0x4dc`.
So a street's house numbers = (owner element, value, parity bitvectors, range) — plus optional exact points in `PA`.

**Attr-vector block layout (CONFIRMED + decoded, 2026-09).** A GenAttr block is framed like the name-list
columns (so it reuses the §11.4 codecs) but has **no trie** (that is `NLAsfBlock` only):
```
@block_off:  u16 num_desc                         (e.g. 0x0031 = 49 in POL block 0)
             num_desc × descriptor { u16 kind, u16 code, u32 data_off, u32 param }   (12 B each)
             ...block-relative sub-stream data...
```
`NLGeneralAttributeBlock::SetDataBlock` (`00e09b60`) splits each `kind`: **`kind & 0xfff`** selects the
attribute-vector slot (the `+0x28 … +0x4dc` offsets cited above), **`kind & 0xf000`** selects that vector's
sub-stream (`0`=primary, `0x4000`=secondary, `0x8000`=tertiary) — the very same sub-stream-flag scheme as the
name-list columns. Empirically on `POL/LID40006` block 0 (49 descriptors, `elem 0..7903`):

| col (`kind&0xfff`) | attr-vector | observed (`POL` block 0) |
|---|---|---|
| `0xc01` `0xc11` `0xc12` `0xc13` | `NLValueListAttrVector<u32>` | owner index (values = ascending street-elem ids), HNR value lists (`15690,15691,…` / `16408,15884,…`) |
| `0xc03`–`0xc06` | `NLValueListAttrVector<u8>` | HNR token/addition strings (mostly empty here) |
| `0xc02` | `NLSingleValueAttrVector<u32>` | one optional scalar |
| `0xc08` `0xc10` `0xc14` | `NLRangeAttrVector<u32>` | range low/high pairs (`576,39,0,56,…` interleaved) |
| `0xc09` `0xc0a` `0xc0b` `0xc0c` `0xc0d` `0xc0e` | `NLBinaryAttrVector` (bitmap) | parity/flags: even-parity **6067/10886 set**, odd **6109/10886** |
| `0x001`–`0x005` | `NLCellIdAttrVector` | cell ids (`155584,155585,…`) + delta/value sub-streams |

Each attr-vector decodes a small set of sub-streams at block-relative descriptor offsets, using the §11.4
primitives:
- `NLSingleValueAttrVector<u32>` (`00cddd74`) = bitfield (has-value bitmap over element_count) + `NLStandardDecoder`
  of `popcount` values — i.e. one optional scalar per element (same shape as the belonging/position columns).
- `NLRangeAttrVector<u32>` (`00e07850`) = bitfield (has-range) + two `NLStandardDecoder` value streams (range
  low / high), each `popcount`-compressed.
- `NLValueListAttrVector<u32/u8>` (`00cddb38` / `00e0be60`) = bitfield (element present) + a count vector + an
  `NLValueListDecoder` (`00cdd728`) stream that flattens the per-element variable-length value lists.
`enDecodeHnr` calls these columns in a fixed order (offsets above). **Status: the GenAttr *container +
TOC + block interior* are now decoded** (`NLGenAttrFile` container + `NLGeneralAttributeBlock` descriptors +
attr-vector streams — `src/lid_format::{read_gen_attr, GenAttrIndex::decode_block}`, validated against
`POL/LID40006` block 0, `#[test] gen_attr_stock_block0_decodes`). What remains is the **HNR writer** and the
on-device pattern-matching (`NLHnrToTree` / `NLHnrVerification` `00ce76a8`) semantics, which cannot be run here.

**Stream framing + byte-exact write proof (2026-09, `LID_format.md §11.6` / `src/lid_format/src/rebuild.rs`).**
`SetDataBlock` is *streaming*: a descriptor row `i`'s **byte span is `[off[i], off[i+1])` in table-row order**
(the last row to the block end); rows sharing an `off` make the *earlier* a zero-length alias view. The
`u32 param` is the *logical element count* the device's decoders consume — **decoupled from byte length**.
Dispatch is by `kind & 0xfff` (vector slot — cols 1/2/4/5→CellId, 1/0xc01/0xc11/0xc12→ValueList\<u32\>,
0xc03-0xc06→ValueList\<u8\>, 0xc02→SingleValue\<u32\>, 0xc08/0xc10/0xc14→Range\<u32\>, 0xc09-0xc0e→Binary,
>0xc13→ValueList\<pair\>) and `kind & 0xf000` is that vector's *member* selector, **not** a separate codec —
and a numeric stream's `code` is always ≥ `0x11` while bitfields (`0x01` dense/`0x02` sparse-set/`0x03`
sparse-clear) use `0x01-0x03`. **Proof the encoders are byte-faithful (no card needed):** ignored test
`gen_attr_stock_rebuild_byte_exact` rebuilds **every byte** of `POL/LID40006`'s 27 MB across **all 133
readable blocks** from our decoded values (the descriptor table + inter-region padding are preserved
verbatim; the 134th TOC entry's `block_off` `0x4c010031`≈1.275e9 is past EOF — the device can't read it
either, so the oracle skips it). Confirmed encoders: raw-u32 (`0x11`), VLE (`0x14`), cumulative-VLE delta
(`0x16`), Simple9 greedy-largest-count (`0x18`, mode table `1:(28,1)…9:(1,28)` matches author exactly),
run/boundaries (`0x12` u32 / `0x15`,`0x17` VLE of run-ending boundaries), bitmap (`0x01` LSB-dense / `0x02`
sparse-set / `0x03` sparse-clear, both as delta-VLE over `param` bits). So *emitting* our data through these
 same encoders yields device-correct bytes; only choosing each column's `param`/`off` + the street↔record join
for OSM `addr:housenumber` remains (see §11.8).

**GenAttr *writer* + join model derived from firmware (2026-09, `src/lid_format/src/write.rs` + reader).**
Ghidra (`enGetHnr 0xe0d078`, `enGetHnrOwnerDescrIndices 0xe0c218`, `enGetOwner 0xe0bdd0`,
`SetDataBlock 0xe09b60` selector switch `0xe09bb0..`) pins the per-record member layout and the join. A record
= **one address element** (the `elem_start..=elem_end` range); members read for the HnR lookup:
`+0x28` VL\<u32\> selector `0xc01` = **per-street list of address-element ids** (existence/ `4000` bitmap,
`8000` = *cumulative offset* deltas, `0000` = the concatenated ids) — this is the street→addresses inverse the
UI browses; `+0x7c` selector `0x002` SV\<u32\> (per-addr u32); `+0x260..+0x300` `0xc09..0xc0d` `tBitArray`
(parity/refuse-interp/interp); `+0x300` `0x00c` `Range<u32>` (the **house number** `from`=`8000`/`to`=`0000`)
read via `enGetValue(+0x300,…)`; `+0x47c` `0xc12` `Range<u32>` per-addr **owner-descr index** → `enGetHnr-
OwnerDescrIndices` → `+0x4d0` `0xc13` VL\<u32\> = **per owner-desc the street-elem list** (`enGetOwner`) = the
record→street join, and `LISA_…` then resolves each street-elem id back to LID2 for the result set. Param rule
(author): `flag 4000`.param = *domain* (existence bit count), `flag 8000`.param = Σ/total or #exist,
`flag 0000`.param = #values; each member's *own* `8000` param equals its `4000` popcount. The synthetic
`#[test] gen_attr_writer_roundtrip_device_model` writes 2 tiling blocks and asserts, on re-parse,
`rebuild_all` byte-equality **and** the enGetHnr/enGetOwner-style member lookups return the exact encoded
street→addr / addr→number / addr→street answers. **Still needs** the OSM-→element join in `osm2lid`: map each
`addr:housenumber` point is now wired: `osm2lid` collects those objects, joins them to **the street**
`LID20006` element named by `addr:street`, and feeds `BlockData{0xc01 street→addr elem, 0x002 addr→street
Range, 0x00c number Range(from=to=num), 0xc0a parity bits}` to `write_gen_attr_file` (§11.6b end / §12.10).
Stock keyed some of these columns through **LID3 crossing** element/owner-descr ids we do not generate; the
author join semantics on-device (`NLHnrToTree`) are unverifiable without the card, so our columns are the
reader-consistent subset validated offline: 5 `gen_attr_*` tests (rebuild oracle + device read-model) and
`osm2lid/tests/genattr.rs` (real binary, real fixture, columns re-read with `read_gen_attr`).


**GenAttr is a *different* container — `NLGenAttrFile`, not `NLNameList` (RESOLVED 2026-09; earlier
"same container, garbage section table" note was WRONG).** Both are opened via
`NLDataBlockReader(fileType 0x13)` (`poGetNewNLGenAttrProcessor` `00bd40ac`), so the 38-byte outer header is
shared, but GenAttr files are parsed by `NLGenAttrFile::DecodeSubHeader` (`00e0e3d0`), NOT
`NLNameList::LoadHeader` — which is why the name-list section-table decode returned garbage on them. Real
layout (verified on `POL/LID40006`):
```
@hdr(=u32@0x10):  u32 elem_count, u32 x, u32 toc_count, u32 other_count   (other_count=1739 here)
@hdr+16:          toc_count × NLBlockTocEntry { u32 elem_start, u32 elem_end, u32 block_off }
                  (element ranges tile [0, elem_count) contiguously — the reliable GenAttr signal)
@block_off:       NLGeneralAttributeBlock: u16 num_desc (=0x31 in block 0, the `31 00`), then num_desc ×
                  12-byte {u16 kind,u16 code,u32 off,u32 param} descriptors, then column data. No trie.
```
`POL/LID40006`: `elem_count = 974 871` (**numerically identical to street base `LID20006`**),
`toc_count = 134` blocks covering ranges `0..974 870`, `block_off ≈ 206 KB` apart. `src/lid_format` detects
this via `is_gen_attr` (validated contiguous tiling), indexes it with `read_gen_attr`, and decodes each block's
column streams with `GenAttrIndex::decode_block`. Note the block columns index a *record* space whose per-block
count is the descriptors' `param` (e.g. block 0 bitmaps are `10886` bits) — which can exceed the TOC range size
(`0..7903`); the owner VectorList column `0xc01` (values ascending within the block range, e.g. `2,2,3,6,7,11,…`)
joins each record → its street element. The per-street **HNR record↔element join** and `NLHnrToTree`
pattern semantics are the last thing not pinned (the on-device matcher cannot be run here).

### 11.7 `PA_%05u` (point addresses) and `REL_%05u` (relations)

- `PA` (fileType 0x18, `NLPABlock::SetDataBlock` `00e0f0b4` / `bDecodeLists` `00e0f824`): columns
  owner-street-elem (`0xd04` SimpleList<u32>), flags (`0xd05/6`), **HNR value** (SimpleList<u8>), and an exact
  `NLPositionAttrVector` → resolves a house number to a real coordinate (no interpolation).
- `REL` (fileType 0x16): a **relation matrix** (`NLRelMatrixFile` `00e11874`), logically
  `multimap<(source_elem, target_elem, type)>`; city→street and city→district(type 2). Exact disk bytes
  still **INFERRED** (matrix sub-header/block `00e11874`/`00e10fbc` to pin).

### 11.8 Converter verdict (CONFIRMED for the address path)

To emit a working **regional** address search you must produce: `LID%05u` (name-list), `LID%05u@(id+10000)`
(crossings), `LID%05u@(id+20000)` (GenAttr/HNR), `PA_%05u` (point addresses), and the SQLite global city list
(`GLOB_POI.DAT` and/or `DB_CITY.DAT`). **`META%04u` and `CONNECT.DAT` are NOT required for regional address
search** (they supply the language table + cross-region thesaurus/linkage; omitting them only degrades
multi-language fuzzy matching and cross-region aliases). **The columnar writer (VLE/Simple9/position columns)
is the real implementation cost** — build `lid2dump` first and decode stock before attempting the writer.

> **Status.** `lid2dump` (reader) dumps the SQLite half (`GLOB_POI.DAT`) to JSON, and — via the new
> `src/lid_format/` crate — now **decodes the ASF `LID2*` name-list blocks** (§12): container, block table,
> descriptor TOC, column codecs, `outDegree`/DFS trie, edge labels, positions, belonging. Verified on
> `POL/LID20006` (98 blocks / 974871 elements; real street names surface). `osm2lid` (writer) emits the
> SQLite `GLOB_POI.DAT` **and** the ASF **city + street name-lists** as a plain trie (`LID20000.DAT`,
> `LID20006.DAT`, §12.9), both round-trip validated. The GenAttr (`LID4nnnn`, +20000) **container + block TOC**
> are now decoded too — container, block TOC **and the block interior** (`NLGenAttrFile` /
> `read_gen_attr` / `GenAttrIndex::decode_block`, §11.6; validated on `POL/LID40006` block 0) — the reader no
> longer returns garbage on them and the descriptor/attr-vector structure + parity/owner/value columns decode.
> **GenAttr writer DONE (2026-09):** `src/lid_format::write` + `osm2lid` `write_gen_attr` emit `LID40006.DAT`
> (OSM `addr:housenumber` objects joined to their `addr:street` street element); encoders are the byte-exact
> ones proven over `POL/LID40006` and the writer round-trips through the reader + a device read-model
> (`gen_attr_writer_roundtrip_device_model`, `osm2lid/tests/genattr.rs`). **Still pending:** `NLHnrToTree`
> on-device matcher semantics (no card to run it), crossing files (+10000), `PA`, `REL`.
> No `DB_CITY.DAT` on this card, so SQLite cannot substitute for the regional city name-list — it must be
> written as `LID20000.DAT`.

### 11.9 Empirical reality on the EUR card — **[CORRECTED, 2026-09; earlier "no sample" claim was WRONG]**

> ⚠️ A previous revision of this section claimed the `NLNameList` ASF container does not exist on the EUR
> card and was therefore a "BLOCKER". That was **incorrect** — it was based on scanning only `LID0*`/`META*`
> in the POL region. It is now **disproven**: the ASF name-lists are present in **every** region. See §12.

Ground truth (verified 2026-09):
- Every region has ASF name-list files `DATA/DATA/LID/CCP/<reg>/LID2nnnn.DAT` (e.g. POL `LID20000..LID20011`,
  `LID20006.DAT` = 15.3 MB, `element_count = 974871`). These match the `NLNameList`/`NLAsfBlock` container
  of §11.2–11.7 exactly (verified by a working reader — see §12).
- `LID0nnnn.DAT` is the `fm_tcl` **content/landmark** container (§5), *not* the address name-list — that is
  why an `NLNameList` header scan against `LID0*` finds nothing.
- **`DB_CITY.DAT` does NOT exist on this card** (verified: the only SQLite DBs are `GLOB_POI.DAT`, table
  `GLOBAL_POIS`). So `LISA_SQLiteGlobalAccess::bGetGlobalCityList` (`SELECT … FROM GlobalCityList`) finds no
  table here → the *global* city DB path is inactive on this edition. **Regional city/street/HNR search is
  served entirely by these ASF `LID2*` name-lists** (via `NLProcessor`), which we must therefore be able to write.

---

## 12. The address name-list is a per-block **trie** (`NLAsfBlock`) — **[DECODED; block geo-origin UNKNOWN]**

The `LID2nnnn.DAT` files are read by `NLProcessor` (`bGoToElemet` `00cef…`, `enGetAllElementProperties`
`00…`, `copszGetCurrentString` = the accumulated `NLProcessor+0x12c` string). A file = an `NLNameList`
container; each block is an `NLAsfBlock` that encodes a **name trie (a plain tree — NOT a minimized DAWG:
empirically every node has exactly one parent, `Σ outDegree = node_count − 1`, 0 multi-parent nodes)**
plus per-element columns. The block decodes into (all CONFIRMED from accessor bodies unless marked):

**12.1 Container** (`NLNameList::LoadHeader` `00e0e63c`) — as §11.2. Sub-header `@hdrSize`:
`u32 element_count`, `6×u16` (vec sizes; `[0]`=block count), `2×u32`, then `7×{u8 section_code, u32 file_off}`.
Section table gives a **block table**: section w/ `code=0x11` = raw `u32` block **file offsets** (monotonic,
cover the file); section `code=0x14` = **decoded byte-sizes** per block (VLE). Verified `POL/LID20006`:
98 blocks, block0 @ `0x362`, Σ decoded-sizes ≈ `element_count` × ~102 B.

**12.2 Block** (`NLAsfBlock::SetDataBlock` `00cdf404`) — 8-byte header `4×u16`: `[0]=node_count`, `[1]=x`,
`[2]=element_count`, `[3]=num_desc`. Then `num_desc × NLPSFListDescriptor` `{u16 kindWithFlags, u16 code,
u32 data_off (block-relative), u32 param}`. `enSetListDescriptions` `00cdc0c0` dispatches on `kind & 0xfff`
and **the top nibble `kind & 0xf000` selects the column's sub-stream**: `0x0000`=primary, `0x4000`
=secondary (`+0x10`), `0x8000`=tertiary (`+0x24`). So each multi-part column (position/value) has one
descriptor per sub-stream. CONFIRMED mapping in the `SetDescription` bodies (`00cdb42c`–`00cdc090`).

**12.3 The tree** (`NLAsfBlock::ProcessNode` `00cdbf88`, `CalculateFirstEdgeIndex` `00cdc018`) — CSR/DFS:
- Column **`0x401`** = `NLSimpleList<u16>` = **`outDegree[node]`** (# child edges per node). `Σ outDegree =
  node_count − 1`.
- Children of node `n` are the **contiguous node range** `[childStart[n] .. childStart[n] + outDegree[n])`,
  where `childStart[n] = base + edgeCounter(n)` and `edgeCounter(n)` is the **DFS-preorder** edge counter
  (not a node-index prefix sum!). `CalculateFirstEdgeIndex` drives it: `base` starts at `1`, the outer loop
  runs `ProcessNode(node, base, &edgeCounter)` advancing `node += (subtree node count)` and `base += 1`
  (a plain single-rooted trie makes the first call cover the file, so `base≡1`, `childStart[n]=1+preorder_edges(n)`).
  Using the **BFS prefix sum** `Σ_{k<n} outDegree[k]` instead of this DFS-preorder counter mis-parents nodes
  and concatenates sibling street names — the earlier bug. Node 0 = root.
- It is a **plain trie, not a DAWG** — verified on `POL/LID20006` block0: `max_parents=1`, `orphans=0`,
  `multi_parent=0`. So `name_of[node]` via the unique parent chain is well-defined.
- **Terminating elements** (`CalculateTerminatingElementIndex` `00cdbf48` → `ProcessSubTreeTEIC` `00cdbe58`):
  a **`element` = a leaf node (`outDegree==0`) with no block-link**; assigned in a specific **DFS**: for each
  node, first its leaf children (in child order), then recurse into its internal children (in child order).
  `+0x8c[element] = leaf node`; `enGetTerminatingElementIndex` binary-finds the node in `+0x8c`. **The element
  index = rank in that DFS order** — all per-element columns (position/belonging/flags) are indexed by it.

**12.4 Edge labels / names** (`NLEdgeLabelList::Decode` `00cdf2f0`, column **`0x403`**) — two sub-streams:
- The **raw** sub-stream (code `0x11`, at object `+0x10`) = a **name blob** of `param` bytes = concatenation of
  per-node labels. The **other** sub-stream (VLE, at `+0x0`) = **absolute per-edge byte offsets** into that blob
  (`count = node_count−1`, monotonic, `last = blob.len`). Node `n`'s label = `blob[off[n-1] .. off[n]]`
  (root `n=0` = empty). A node's **name = concatenation of labels root→node** (path labels).
- Verified on `POL/LID20006` block0: labels are first the alphabet (`" ' 1 2 … A B … Z`, 1 char each, = root's
  children) then **full-name phrases**. A leaf path spells one street name.
- **Name variants are separated by `0x09` (tab):** typically `<ASCII-folded> \t <proper-UTF8 w/ diacritics>`
  (e.g. `"ZOSKA", ULICA BATALIONU \t "ZOŚKA", …`). The display name is the diacritic-bearing variant; the
  ASCII fold is a fold of it. `decode_name` splits on `0x09` and returns the variant with non-ASCII bytes
  (else the longest). Both variants appear as text — sometimes one element with an embedded `0x09`, sometimes
  as two sibling elements. This is the store's alt/main name encoding (the `enGetEntry*Name*` accessors).
- Column **`0x8403`** (flags `0x8000`, VLE) = the **charset codebook** (byte-code → character incl. multibyte).
  In `POL/LID20006` labels are literal bytes (UTF-8 decodes correctly), so the codebook is not needed to
  reproduce these names; **UNKNOWN (minor):** when a charset-code mapping would apply.
- **Descriptor stream lengths:** `NLAsfBlock::enSetListDescriptions` `00cdc0c0` computes each stream as
  `len = off[next_row] − off[this_row]` in **descriptor-row order** (the next row's `off`, not the next
  *distinct* one), so equal-`off` rows = a zero-length view. The byte-exact rebuild oracle matches this rule;
  `read` now uses it too (the old "next strictly-greater `off`" rule mis-assigned bytes for tied rows).

**12.5 Positions** (`NLPositionAttrVector::Decode` `00cdd7dc`, column **`0x407`**) — CONFIRMED:
`flags=0x4000` sub-stream = **bitmap** (`NLBitfieldDecoder`, code `0x03` = VLE-clear) over `element_count` of
which elements have a position; `flags=0` sub-stream = **COMPRESSED interleaved `X,Y` `u32`** — only `2·popcount`
values, and element `i` (bit set) maps to `coords[2·rank]`,`coords[2·rank+1]` where `rank` = set bits before `i`
(`Decode` stores an index vector `= rank<<1` at `+0x20`, `+0x40`=`popcount`). Element position =
**block origin + (X, Y)**. Verified: values ≈ PAU deltas from origin (first block0 entry `X=233844` ≈ +0.0196°).
**RESOLVED (2026-09 — anchor fit on stock `POL/LID20004` + `LID20006`):** the per-block `tNLHPosition` is **NOT
in the file** — it is the position of the **city being searched**, passed by the address-search client when it
loads the block (`NLProcessor::enAddNewAsfBlock` `00ceca4c` → `enReadAsfBlock` `00cf4f70` →
`NLAsfBlock::SetDataBlock`; the container's per-block `sub+0x20/+0x30` `u32` vectors are **RAM** query state,
which is why they never appear in the block bytes). Proof: for unique street anchors the residual
`truth − stored` is constant **per city** and equals `city_position − file_corner` for Warsaw/Kraków/Kielce/
Łódź (±street spread ≈ 1 km); a search for those `u32` city pairs anywhere in the name-list file finds nothing.
**Stored street coordinate = element position − queried-city position.** The file-level `tNLHPosition` lives in
the sub-header (`hdr+0x0c` bit `0x0008_0000` = present, `hdr+0x10/0x14` = `X,Y`; on `POL` the values are the
region's SW corner ≈ 14.12°,49.10°; `LID20000` sets the bit but stores `-1/-1` = "no positions"). It is the
anchor used by flows **without** a city context (e.g. the settlement gazetteer), so the **writer contract** is:
one city per block, elements stored as `pos − that_city_position` (the coordinate the device resolves for the
city), file header = region corner.

**12.6 Hierarchy (city↔street) + element properties** — CONFIRMED accessors, indexed by `element_index`:
- **belonging name** = parent/city element: `enGetEntryBelongingNameMainElementIndex` `00cdba88` →
  `NLSingleValueAttrVector<u32>` @`+0x328` (column **`0x40c`**: `flags0`=bitmap "has-belonging",
  `flags0x4000`=values).
- per-element flags: `bIsEntryValidDestination` / `bHasEntryCrossing` / **`bHasEntryHouseNumber`** /
  `bHasEntryPointAddresses` / `bHasEntryCells` / `…DetailedDescription` (bitvector columns; `enGetEntryCharacterStatus`
  → column `0x415` `NLBinListAttrVector`).
- name variants: `Permutation`/`Alternative`/`Belonging`/`Exonym`/`Base` (`enGetEntry…NameMainElementIndex`).
- `0x402` = `NLBlockLinkAttrVector` = bitmap + `2·k` u32 pairs → `map<node, {block, index}>` (cross-block links).

**12.7 Converter verdict.** To emit a working regional address search (city+street+HNR) we must write
`LID%05u` (cities/streets name-lists) + `@(id+20000)` GenAttr/HNR + `PA_%05u` (+ `REL`), as this **trie/column
format**. Stock is already a **plain trie** (§12.3), so the writer builds a trie directly — insert each full
name root→leaf (terminating each with `0x00` so strict-prefix names stay leaves), number nodes in the
DFS-preorder `childStart` layout, split into ≤ ~10k-node blocks (`node_count`/`element_count` are `u16`), and
populate the terminating-element DFS order + the per-element columns. No DAWG minimization needed. House
numbers = per-street GenAttr `+20000` columns (§11.6); optional exact points in `PA` (§11.7).

**12.8 Reader status.** `src/lid_format/` (Rust, wired into `lid2dump`) implements §12.1–12.6 faithfully (no
heuristics/filters): container + block table + descriptor TOC (flag sub-streams) + column decoders (§11.4,
**correct** Simple9 mode table §12.4 and custom `ReadVle`) + `outDegree`/DFS-`childStart` children +
`CalculateTerminatingElementIndex` element DFS + `0x403` blob/offsets + `0x09` name-variant split +
`NLPositionAttrVector` (bitmap + rank-compressed PAU) + belonging column. On `POL/LID20006` it decodes
**883 936 elements / 883 929 with positions / 231 882 unique names**, all real Polish street names with
diacritics. The former "only remaining UNKNOWN" (the per-block **geo origin**, §12.5) is **RESOLVED**: stored
coords are deltas from the queried city's position (query-supplied), and `read` also exposes the file-level
`tNLHPosition` (`NameList.origin`). Verified it is a pure trie (single-parent, all nodes reached).

**12.9 Writer status.** `src/lid_format::encode` (Rust) is the writer half of the oracle: it builds the trie,
numbers it in the same DFS-preorder layout, splits into ≤10k-node blocks, and emits the container + descriptor
TOC + raw/VLE column streams — the exact bytes `read` consumes. `osm2lid` now writes **all three** address
files (unit tests + `malopolskie` fixtures):

* **streets** (`LID20006.DAT`): one element per unique highway `name` at its way centroid, absolute PAU,
  block origin 0 — **8828 names → 14 blocks → 8828 elements back**. NOTE (§12.5): device-correct output needs
  **city-grouped blocks** with positions stored **relative to the owning city's position** — currently the
  writer emits absolute PAU with a null origin, so rework pending.
* **cities/settlements** (`LID20000.DAT`): one element per unique OSM `place=` name — **10482 names → 12
  blocks → 10482 elements back**, diacritics intact.

**12.10 House-number GenAttr writer (`LID40006.DAT`).** Same crate, different container (§11.6b). `osm2lid`
collects `addr:housenumber` objects (nodes or building/entrance ways; the street comes from `addr:street`,
falling back to `addr:place`) that land within `--bbox`, keeps **numeric** numbers only (the authoring tool
too dropped suffixed/compound numbers such as `11A` — they cannot live in the `u32` number column), and joins
each to the *same* street-element order as `LID20006.DAT`. It chunks the address records
(`HN_ATTR_CHUNK`=8192, adaptive so even tiny inputs emit the ≥ 2 TOC blocks the container requires) and writes
the four device columns: `0xc01`(+0x28) street→address **element-list** (existence `4000` over the used-street
domain, cumulative `8000`, `0000` element ids in sorted-street order), `0x002`(+0x7c) address→street (Range
from/to = street id), `0x00c`(+0x300) the house **number** (Range from/to = the number; `8000`=VLE-delta of
the froms so their decode-cumulatives are exact, `0000`=raw-u32 tos), and `0xc0a`(+0x280) per-address
**parity** (even-number bit ride in the column's existence bitmap). Validation is offline (no card in the
loop): `lid_format`'s `gen_attr_*` tests (byte-exact rebuild oracle over `POL/LID40006` + device read-model
round-trip) and `osm2lid/tests/genattr.rs`, which runs the real binary on a tiny fixture and re-reads every
column with `read_gen_attr`/`decode_block`. **Still pending:** `PA`, `REL`, crossing files (+10000), and the
city-grouped / city-relative positions in `encode` (§12.5); the on-device `NLHnrToTree` matcher itself cannot
be run here.

> **Corrected decoder notes (were wrong in earlier revisions):**
> - **Simple9 mode→(count,bits)** (from `DecodeSimple9` `00cdc3bc`/`00cdc908`, values LSB-first, mode nibble
>   `= word>>28`): `1:(28,1) 2:(14,2) 3:(9,3) 4:(7,4) 5:(5,5) 6:(4,7) 7:(3,9) 8:(2,14) 9:(1,28)`
>   (u16 variant: mode `9`→bits 16). Earlier "mode 1 = 1×28" (Lucene convention) was **reversed**.
> - **`ReadVle` `00cdb218`** is a custom LEB, not standard: continuation byte → `acc = (byte-0x7f) + prev*128`,
>   final byte → `value = byte + prev*128` (i.e. each continuation contributes `128·(chunk+1)`), max 5 bytes.
>   `ReadUnsigned<u32>` = plain LE u32; `ReadUnsigned<u16>` = LE u16.

---



---

## 9. Decoded vs not-decoded — summary & next steps

**Decoded / reproducible now:**
- File organization (regions × families + special files).
- `GLOB_POI.DAT` fully (SQLite FTS3 schema, PAU coords, `REGION_ID`, `CAT_ID`) — trivially writable.
- **CPRNAV_2 decompression for both header widths** (`block_size = unknown×0x400`; 16-bit vs 32-bit
  per-block sizes) → `src/cprnav_decompress/` unpacks every compressed LID and MAP/IDX file.
- Common raw-file container shape (copyright/date/named sections) and the `TPLID_EQUIVALENT_CHAR` table.
- **LID content-block structure** (§5): block header (`fm_tclStartBlockAccess`, bounding box @+0x18, version
  @+5), POI sequences, the point-POI record (`fm_tclPOIData`: display_scale@+6, PAU lon/lat @+0x0c/+0x10,
  text_id@+0x14), the text pool, and the ~16 POI categories (→ `CAT_ID`).

**Not fully decoded (active frontier = the ASF address name-list, §12):**
1. **Block geo origin** (`tNLHPosition`, §12.5) — **RESOLVED**: per-block value = the *queried city's*
   position (client-supplied at block load; RAM-only), file-level value = sub-header corner
   (`hdr+0x0c/0x10/0x14`). No file table to find.
2. **Charset codebook** (`0x8403`, §12.4) — when a byte is a charset code vs a literal (labels decode as literal
   UTF-8 on `POL`, so not exercised yet).
3. **Writer (`osm2lid` + `lid_format::encode`)** — DONE for the **street + city name-lists** (plain trie,
   multi-block, §12.9): OSM highway `name` → `LID20006.DAT` and OSM `place=` → `LID20000.DAT`, both validated
   by full round-trip, **plus the house-number GenAttr `+20000` file** (`LID40006.DAT`, §11.6/§11.6b,
   OSM `addr:housenumber` → street-joined records, validated by `osm2lid/tests/genattr.rs`).
   **Still pending:** point-address `PA` / `REL`, crossing files (+10000),
   and the city-grouped / city-relative position encoding in `encode` (§12.5).

> The trie **structure**, **element order** (`CalculateTerminatingElementIndex`), **edge labels/names**
> (`0x403` + `0x09` variant split) and **relative positions** (`NLPositionAttrVector`, rank-compressed) are all
> CONFIRMED and reproduced faithfully by `src/lid_format/` (883 936 elements off `POL/LID20006`).

> Practical note for a converter: if your goal is *navigation*, LID is the optional content layer — the
> network still loads and routes via RNW→MAP without it. If you need POI search / landmark rendering, the files
> now unpack cleanly **and** the point-POI records are decoded (§5), so a POI exporter can be built directly.
