# Per-level object statistics — region N6E2

Empirical census of what kinds of objects live at each zoom **level** of a TravelMap `.MAP`
dataset. Companion to [`MAP_format.md`](../02%20-%20details/MAP_format.md) (format reference) and
[`writer_guide.md`](./writer_guide.md) (write path).

> Region: **N6E2** (Poland/Czech border area, lon 18–36, lat 47.25–56.7). N6E2 ships **L1/L2/L3**
> only — there is no L4 data for this region. Other regions will have different absolute counts;
> the *shape* of the distribution (which categories appear at which level) is what matters here.

---

## 0. How these numbers were produced

1. Convert the full region, all levels, no bounding box:
   `map2osm_rs <MAP>/N6E2AA.IDX -r N6E2 -l 123 -o <out>` → one `<REGION>_L<level>.osm` per level.
2. Stream each `.osm` and tally every object by `tm:kind`, `tm:layer`, and the low byte of
   `tm:feature` (the category; the high byte is only display scale — see format §7).
3. Cross-check category meanings against object names and the POI/landuse tables in format §7.

A level is a tile-grid granularity, not a separate file: `SHIFTS = [13, 10, 7, 4]` for L1..L4.
Higher shift ⇒ larger tiles ⇒ coarser view. So **L1 = overview, L2 = regional, L3 = street-level**.

---

## 1. Level overview

| metric | **L1** (overview) | **L2** (regional) | **L3** (detailed) |
|--------|------------------:|------------------:|------------------:|
| objects total | 9 122 | 486 781 | 4 313 863 |
| polygons | 4 974 | 180 047 | 3 354 123 |
| lines | 2 613 | 221 470 | 307 799 |
| POIs (points) | 1 535 | 85 264 | 651 941 |

---

## 2. Polygons — terrain class by level

Category = low byte of `tm:feature`. OSM tag is what the converter now emits (format §7).
`0x9C` and `0x38` are unnamed and mixed, so their tags are best-effort; the exact code stays in
`tm:feature`.

| feature | terrain | OSM tag | L1 | L2 | L3 |
|---------|---------|---------|----:|----:|----:|
| `0x9C` | urban blocks (city fabric) | `landuse=residential`* | – | – | **2 780 450** (83%) |
| `0x38` | rural open land | `landuse=grass`* | 1 114 | 101 464 | 244 540 |
| `0x2B` | forest / woodland | `natural=wood` + `landuse=forest` | 2 133 | 21 614 | 171 157 |
| `0x48` | water body (lake/pond) | `natural=water` + `water=lake` | 1 672 | 27 743 | 95 034 |
| `0x39` | cemetery | `landuse=cemetery` | – | 18 222 | 40 793 |
| `0x3A` | commercial / shopping | `landuse=commercial`* | – | 7 049 | 11 335 |
| other | misc. special areas | *(left to `tm:feature`)* | 69 | ~3 800 | ~10 000 |

\* best-effort (see note above).

Polygon name coverage: **L1 39% / L2 28% / L3 4%** — the more detailed the level, the more
polygons are anonymous geometry (individual blocks/parcels) rather than named areas.

---

## 3. Lines — layer by level

| layer | meaning | L1 | L2 | L3 |
|-------|---------|----:|----:|----:|
| `water` | rivers / streams / ditches | 317 | 150 440 | 230 906 |
| `road` | road centreline (has road attrs) | 2 240 | 70 504 | — |
| `line` | generic line, no road/water class | 56 | 526 | 76 893 |

**Roads:** centrelines with road attributes appear in the `.MAP` at L1 and L2 only. At L3 the
detailed street network is **not** in the `.MAP` — it lives in the separate `.RNW` files
(`rnw2osm_rs` / `rnw_extract_rs` pipeline). The L3 `line` objects are the MAP-side references
that carry no road class of their own.

---

## 4. POIs — feature code by level

Category = low byte of `tm:feature`. Meanings verified against object names and the official icon
taxonomy in `POI_MAPPING.DAT` (format §7 / §7.1).

| feature | meaning | OSM tag | L1 | L2 | L3 |
|---------|---------|---------|----:|----:|----:|
| `0x01` | settlement (city/town/village) | `place=*` (from size class) | 348 | 83 389 | 83 401 |
| `0x03` | motorway rest area | `highway=*` (services) | 1 187 | 1 188 | 1 188 |
| `0x14` | shop / supermarket | `shop=supermarket` | – | – | 189 176 |
| `0x10` | school | `amenity=school` | – | 337 | 90 921 |
| `0x15` | bank / ATM | `amenity=bank` | – | – | 49 332 |
| `0x13` | pharmacy / hospital | `amenity=pharmacy` | – | – | 43 976 |
| `0x06` | restaurant | `amenity=restaurant` | – | – | 42 459 |
| `0x16` | church / place of worship | `amenity=place_of_worship` | – | – | 36 148 |
| `0x07` | car service / shop | `shop=car` | – | – | 24 800 |
| `0x02` | parking | `amenity=parking` | – | – | 24 172 |
| `0x17` | leisure / attraction (mixed) | `tourism=attraction` | – | – | 20 634 |
| `0x11` | bar / pub | `amenity=bar` | – | – | 11 694 |
| `0x04` | fuel station | `amenity=fuel` | – | – | 10 981 |
| `0x22` | railway station | `railway=station` | – | – | 8 090 |
| `0x12` | sports / fitness | `leisure=sports_centre` | – | – | 6 087 |

Other minor codes (fuel+brand, junctions, companies, car rental, hotels) round out L3; anything
not in the table is left to `tm:feature`.

POI name coverage is **100% on every level** — points always carry a display name.

---

## 5. What to take away

1. **Amenity POIs exist only at L3.** L1 and L2 carry just settlements (`0x01`) and rest areas
   (`0x03`); shops, restaurants, pharmacies, banks, etc. all appear for the first time at L3.
2. **Urban blocks `0x9C` dominate L3** — 83% of its polygons. This is the detailed city fabric;
   it does not exist as a distinct class at coarser levels.
3. **Settlements are stable across L2/L3** (the same ~83 400 named places); L1 keeps only the
   348 largest.
4. **Roads split by level:** MAP holds centrelines at L1/L2; the street-level network is in `.RNW`.
5. **Water and terrain** (forest / water / cemetery) grow monotonically with level — most detail
   at L3.
6. **Naming inverts with detail:** POIs are always named, but polygon name coverage drops
   39% → 28% → 4% as levels get finer.

---

## 6. Where POIs live (storage architecture)

The per-level census above counts only the **`.MAP`** render set. In the full firmware a POI can
exist in several stores that serve *different* features. All of the following were inspected
directly in the `NISSAN Connect LCN3 V7 2022_2023` image:

| store | format | role | population | read by |
|-------|--------|------|-----------|---------|
| **`.MAP`** | binary (tile/block/cell) | **render** — what is drawn | exhaustive, per-region (N6E2 L3 = 651 941) | `procmapengine.out` |
| **`GLOB_POI.DAT`** | SQLite **FTS3** (`GLOBAL_POIS`) | **full-text search** of named places | curated, global, multilingual (88 871 rows; 44 languages, 16 regions) | Connect UI “search a place” |
| **`POI_MAPPING.DAT`** | SQLite | **taxonomy / config**, *not* instances | `idxCat` (hundreds of categories + brand chains: `AIRPORT=201`, `AMUSEMENTPARK=202`, `7ELEVEN`, `AMERICINN`…), icons, name templates (`neh_1_GenFullPoiNames`, `neh_ShortNameTable`) | label/icon rendering + search results |
| `CCP/ELL/LID*` | binary | **localized display strings** per language (e.g. Finnish) | category/feature labels, no geometry | UI |
| `CONNECT.DAT` | packed `CPRNAV_2` | per-region Connect service content | (unpack with `cprnav_decompress_rs`) | UI |
| **`.RNW`** | binary | **road network only — no POIs** | nodes/edges; the “poi” hits in the extractors are the word *point* | routing |

### The `GLOBAL_POIS` schema (the search index)

```sql
CREATE VIRTUAL TABLE GLOBAL_POIS USING FTS3 (
    IDX NUMBER, NAMENORM VARCHAR(2000), NAME VARCHAR(2000), LANG_IDX NUMBER,
    ORIGINAL_IDX NUMBER, LONGITUDE NUMBER, LATITUDE NUMBER,
    CAT_ID NUMBER, REGION_ID NUMBER, CONTROL NUMBER);
```

- `NAME` / `NAMENORM` — display name and its normalized (upper-cased) search form.
- `LONGITUDE` / `LATITUDE` — **scaled integers, ~10⁷ per degree** (≈1e-8° units up to a ~1.2×
  factor). This is a *different* encoding from the `.MAP` PAU grid — see §7.
- `CAT_ID` — an `IdxID` into `POI_MAPPING.DAT.idxCat` (e.g. `201 = AIRPORT`).
- `REGION_ID`, `LANG_IDX` — which region / language the row belongs to.

### Render vs search — is it duplication?

Yes, in purpose, but **not byte-for-byte**:

- **`.MAP`** holds the *exhaustive per-region* set that gets drawn (every shop, restaurant,
  pharmacy at L3).
- **`GLOB_POI.DAT`** holds a *smaller curated global* catalog of notable places (airports,
  landmarks, brand chains), in many languages. It is a different population: N6E2 alone has
  ~650k render POIs, while the whole search index has ~89k worldwide.
- A POI can be in `.MAP` (visible) but not in `GLOB_POI` (not searchable), and vice versa.
- **`POI_MAPPING.DAT`** is shared *config* referenced by both — it stores no per-location data.

So the two instance stores overlap but are not copies of each other; the third store is config.

---

## 7. Converting OSM → TravelMap POIs (strategy)

Direct answer to “do we write every POI everywhere?” — **no.** Each store gets a different
subset, by purpose:

- **`.MAP`** ← the full set you want *visible* (render).
- **`GLOB_POI.DAT`** ← only a *filtered, searchable* subset.
- **`POI_MAPPING.DAT` / `.RNW`** ← never instances (config / roads).

Work in tiers so each adds value independently:

### Tier 1 — rendering (minimum viable, highest value)

Write OSM POIs into **`.MAP` only**.

- Need the **inverse of `poi_osm()`**: OSM tag → TM feature code
  (`amenity=parking`→`0x02`, `shop=supermarket`→`0x14`, `amenity=restaurant`→`0x06`, …). This is
  **many-to-few and lossy** (OSM has far more amenity/shop values than the ~20 TM codes) — map the
  common ones, bucket or drop the rest. Decide the drop-vs-bucket policy explicitly.
- Emit at the right level(s): local POIs → L3; settlements / rest-areas → L1/L2 (per §4/§5).
- Geometry → PAU (the writer already does this).
- Result: POIs appear on the map. This alone satisfies “I can see my POIs.”

### Tier 2 — search parity (optional)

Also insert rows into **`GLOB_POI.DAT`**, but for a **selected subset only** (do *not* dump all
~650k — that would bloat the index and change search behaviour). Pick notable POIs that match the
existing category scheme (e.g. `aeroway=aerodrome`, `tourism=*`, named chains).

Prerequisites:
- **OSM → `CAT_ID`** mapping into `idxCat` (reuse existing IdxIDs; only extend `idxCat` + icons if
  you introduce genuinely new categories).
- **Coordinate transform** WGS84 → the GLOB integer grid (~10⁷/°; pin the exact factor before
  writing). Note this differs from the `.MAP` PAU transform — two separate conversions.
- `NAMENORM` normalization + `REGION_ID` + FTS table rebuild after bulk insert.

### Tier 3 — routing-to-POI (optional)

Snap each rendered POI to the nearest generated `.RNW` node and store the association so the nav
engine can route *to* it. `.RNW` itself has no POI slot; the snap link lives on the `.MAP`/runtime
side (exact field to be confirmed in the format reference).

### Key risks

1. **OSM → feature-code is lossy** — the drop-vs-bucket decision drives how many OSM POIs survive.
2. **Two coordinate encodings** (`.MAP` PAU vs `GLOB_POI` ~10⁷-int) must each be converted; do not
   reuse one for the other.
3. **Category schemes differ** — OSM `amenity=*`/`shop=*` vs `idxCat` IdxIDs (incl. brand chains);
   a lookup table is required.
4. If search parity is *not* required, **stop at Tier 1** — it is the best value-to-risk ratio.
