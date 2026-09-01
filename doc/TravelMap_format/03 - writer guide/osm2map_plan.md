# OSM XML → MAP/IDX converter (`osm2map_rs`) — plan of action

Companion to [`writer_guide.md`](writer_guide.md) (byte-level how-to), [`MAP_format.md`](../02%20-%20details/MAP_format.md)
(read-path reference) and [`signature.md`](signature.md) (what is / isn't signed). This document is the
**plan**: what to build, in what order, and how to prove each step.

> **TL;DR.** Convert an internet OSM XML extract (e.g. `krzeszowice.osm`) into TravelMap `.IDX`/`.MAP`
> files that **replace** an existing region's files in the car so the OSM roads / POIs / areas render.
> Compression is already solved (`cprnav_compress_rs` / `cprnav_decompress_rs`, verified byte-exact).
> The new work is one crate, `osm2map_rs`, that turns OSM into the **decompressed** `.IDX`/`.MAP` layout
> (the same layout `map2osm_rs` reads), which we then compress for deployment.

---

## 1. Goal & scope

- **Input:** standard OSM XML (`<node>/<way>/<relation>` + `<tag>`). Reference input:
  `/home/marek/Ext/reverse_engineering/NissanMaps/OSM-map/krzeszowice.osm` (Kraków area, ~50.1N 19.6E).
- **Output:** a full set of decompressed `N6E2AA.IDX` + `N6E2<prof>.MAP` files for a chosen region, then
  compressed to CPRNAV_2 and dropped into `DATA/DATA/MAP/` **in place of** the originals.
- **Region parameter:** `-r N6E2` selects which region's files to (re)generate. The region's geographic
  extent (BBox in PAU) is **copied from the existing region file**, so tiles align with what the car
  already expects and no signed directory file (`IDX_CNT.TBL` / `RPITABLE.RPI` / `NAV_ROOT.DAT`) changes.
- **Signature-safe:** only `DATA/DATA/MAP/*` (unsigned base) is touched (§signature.md §7). Rendering does
  not involve the RNW patches, so no patch neutralization is needed for a render test.
  - *Caveat:* replacing `.MAP`/`.IDX` changes file sizes. Confirm `DATA/DATA/MISC/CONTENT.DAT` (signed,
    first 2 KB) does not encode per-file sizes — see signature.md §7.4. If it does, sizes must be kept
    stable or that file is out of reach.

Out of scope for v1 (later milestones): RNW routing writer, "augment existing map" mode, re-signing.

---

## 2. Verified pipeline (the foundation — already proven on real firmware data)

The read path was executed end-to-end on the stock `N6E2` region and works:

```
DATA/DATA/MAP/N6E2AA.IDX   (CPRNAV_2, magic "CPRNAV_2" @ 0x04)
        │  cprnav_decompress_rs          (verified byte-exact)
        ▼
decompressed N6E2AA.IDX    (binOff@0=0x234, spare@2=32, BBox@4, partOff@0x14=126)
        │  map2osm_rs  -l 1 -b 19.5,50.0,19.8,50.2
        ▼
N6E2_L1.osm                (6511 nodes, 760 ways for the Kraków box — correct geometry)
```

Decompressed `N6E2AA.IDX` header (measured):

```
0x00 binOff   = 0x0234
0x02 spare    = 32
0x04 west     = 0x0CCCCCCC  (18.00°)      0x0C south = 0x21999997 (47.25°)
0x08 east     = 0x19999998  (36.00°)      0x10 north = 0x2851EB82 (56.70°)
0x14 partOff  = 126
```

Decompressed `N6E210I.MAP` header (measured):

```
0x00 binOff   = 0x07BC          0x02 infoTbl = 0x34        0x04 fileSize = 0x00149B08 (= exact size)
0x08 BBox     = identical to the IDX
0x18 @0x18    = 8               0x1a @0x1a = 4            0x1e @0x1e = 0x8412 = 0x8400 | prof(0x12)
```

**The write path is the mirror image** and every hard part already exists:

```
krzeszowice.osm
        │  osm2map_rs  (NEW — the only code we must write)
        ▼
decompressed N6E2AA.IDX + N6E2<prof>.MAP     (same layout map2osm_rs reads above)
        │  cprnav_compress_rs                (already built, mirrors the decompressor)
        ▼
DATA/DATA/MAP/N6E2AA.IDX + N6E2<prof>.MAP    (CPRNAV_2 — drop in place of the originals)
```

So `osm2map_rs` only has to emit the **decompressed** layout. Compression, the coordinate model, and a
working reader for validation are all in hand.

---

## 3. Binary format to emit (decompressed)

Full byte reference is in `writer_guide.md` §1–§6; this is the condensed write recipe, cross-checked
against the measured N6E2 headers above.

### 3.1 `.IDX`

```
0x00  u16 binOff            offset of the L0 tile table
0x02  u16 spare             const 32
0x04  u32 west/south/east/north   region BBox, PAU (deg * 2^31 / 180) — copy from the existing region
0x14  u16 partOff           partition-table offset in 4-byte units (ref uses 126)
partOff*4  partition table: 4 × 12 B, one per level i=0..3 (measured on N6E2):
            +0 = i, +1 = latPart[i], +2 = latPart[i]   (both bytes = (1,5,10,10))
            +3..6 = u32a = (tileCnt << 8) | coordShift[i]   // coordShift=(13,10,7,4)
            +7..10= u32b = (tableOffset << 8)        ← where level i's tile table lives
          N6E2 measured: latPart=(1,5,10,10), coordShift=(13,10,7,4), tileCnt=(1,25,2500,250000).
          NOTE the coordinate delta shift (13,10,7,4) is in the u32a low byte; the +2 partition
          byte repeats latPart and is NOT the coord shift. N6E2 has all 4 levels structurally
          (L4 = 250000 tiles), though L4 carries no data for this region.
each tile table: one 8-byte slot per tile K:
            single profile : { u16 (0x400|prof), u16 lenWords, u32 offset-in-MAP }
            several        : multi slot { u16 0x4000, u16 count, u32 ptr } + count×8B sub-slots
            empty          : { u16 0xC000|…, 0, 0 }   (bit15 = empty)
```

`lenWords` is the block size **in 4-byte words** and must equal the MAP block marker length.

### 3.2 `.MAP` (one file per profile: `N6E2<prof>.MAP`, `prof` low byte → base32 pair in the name)

```
0x00  u16 binOff   offset of first data block
0x02  u16 infoTbl  const 0x34 (52)
0x04  u32 fileSize total file size in bytes
0x08  BBox         identical to the IDX
0x18/1a/1c/1e      metadata — copy from a reference profile or use the documented constants
binOff  blocks, contiguous & 4-byte aligned:
            u32 marker = (0xFFFF << 16) | lenWords   // high16=0xFFFF, low16=lenWords
                                                       // (reader checks (marker&0xFFFF)==len; verified 0xffff1438)
            3 × {u16 start, u16 count}   for list 0=polygons, 1=lines, 2=POI
                                         start[0]=4; start[i+1]=start[i]+count[i]*3
            cells (12 B each):
              lists 0/1: {u16 state, u16 feature, u16 pointIdx, u16 count, u32 annotDesc}
              list 2   : {u16 state, u16 feature, i16 dlon, i16 dlat, u32 annotDesc}
            point pool: count × {i16 dlon, i16 dlat} at (pointIdx*4)
            annotations+text: packed {u8 size, u8 type, payload[size-2]} after the point pool
```

`annotDesc = (startWords << 16) | count`. Text/annotation positions are **block-relative** (`v * 4`).

### 3.3 Coordinate & tiling model

- `PAU = deg * 2^31 / 180` per degree of lon/lat (both axes).
- Tiling is a **5-based quincunx**: level i has `5^(2i)` tiles → 1 / 25 / 2500 / 250000.
  Tile `K` at level i covers a sub-rectangle of the region BBox (see `Region::tile_extent`).
- A point is stored as a **signed delta from the tile center**, scaled by `SHIFTS[i]`:
  `coord = center + (delta << SHIFTS[i])`, `SHIFTS = [13,10,7,4]`. Deltas are `i16`, so each object's
  coordinates must fall within `±2^15 << shift` of its tile center (clamp/split if not — see §10).

### 3.4 Profiles & slots (v1 simplification)

- A tile normally references one profile → **single slot**. v1 emits **one profile** per region and uses
  single-entry slots only (no `multi`), which sidesteps the multi-slot complexity. All OSM objects for a
  tile go into that one profile's block.
- The original N6E2 uses five profiles (`102,10E,10H,10I,11A` = prof values `0x02,0x0E,0x11,0x12,0x2A`).
  Replacing the region with a single-profile set is fine for a render test; whether the runtime *requires*
  the original profile set is an M0 check (§10).

### 3.5 `.TCI`

Per-MAP "Tile Cluster Index" — an **optional** fine-grained sub-index, *"not needed to decode the geometry
itself"* (MAP_format.md §, 01_MAP_overview.md). It is also CPRNAV_2-compressed and carries a copyright
string. v1: **omit** (or emit a copy of a reference TCI) and confirm in M0 that the runtime tolerates its
absence for rendering.

---

## 4. OSM → MAP object model (the core new logic)

For every OSM primitive decide: **kind** (`poi` / `line` / `polygon`), **feature code** (u16: low byte =
category, high byte = display scale), **state**, and **name**. The tables below are the **inverse** of the
mappings already implemented and verified in `map2osm_rs` (`poi_osm`, `landuse_osm`, `add_semantic`,
`ann_tags`), so a round-trip through `map2osm_rs` should recover the same categories.

### 4.1 POIs (OSM `<node>`) → list-2 cells

| OSM tag | feature low byte | note |
|---|---|---|
| `amenity=parking` | `0x02` | |
| `amenity=fuel` | `0x04` | + `gas` annotation (type 0x30) |
| `tourism=hotel` / `guest_house` / `hostel` | `0x05` | |
| `amenity=restaurant` / `fast_food` | `0x06` | + `restaurant` ann (0x34) |
| `shop=car` | `0x07` | |
| `office=*` | `0x08` | |
| `amenity=car_rental` | `0x09` | |
| `amenity=school` / `kindergarten` | `0x10` | |
| `amenity=bar` / `cafe` / `pub` | `0x11` | |
| `leisure=sports_centre` / `stadium` / `pitch` | `0x12` | |
| `amenity=pharmacy` | `0x13` | |
| `shop=supermarket` / `convenience` / `mall` | `0x14` | |
| `amenity=bank` | `0x15` | |
| `amenity=place_of_worship` | `0x16` | |
| `tourism=attraction`/`museum`/`viewpoint`/`zoo`/`theme_park` | `0x17` | mixed leisure class |
| `railway=station` / `halt` | `0x22` | |
| `place=city/town/village/hamlet` (named) | city POI + `0x21` city ann | size class from population/admin (§5) |

Unmapped amenities are still emitted as a POI with their name; the exact code stays best-effort and is
refined during M3.

### 4.2 Roads / lines (OSM `<way>`) → list-1 cells + roadinfo annotation

`highway=*` → **netclass** (bits 0–2 of the roadinfo `u16 w`), inverse of the reader's class table:

| OSM highway | netclass |
|---|---|
| `motorway` | 0 |
| `trunk` | 1 |
| `primary` | 2 |
| `secondary` | 3 |
| `tertiary` | 4 |
| `unclassified` / `road` | 5 |
| `residential` / `living_street` | 6 |
| `service` / `track` / `path` | 7 |

Sub-attributes (same roadinfo word): `junction=roundabout` → road_type 2; `highway=*_link` → road_type 9
(interconnect) or 1 (long ramp); `toll=yes` → toll bits; `route=ferry` → ferry bits (emitted as a line, not
a road class). The full roadinfo payload is written as annotation type `0x11` `{u16 w, u32 d}`.

Water lines: `waterway=*` → list-1 with water annotation type `0x10` (class/type nibbles) — `river`,
`canal`, `stream`, `ditch`.

### 4.3 Areas / polygons (closed OSM `<way>` or `relation[type=multipolygon]`) → list-0 cells

| OSM tag | feature low byte |
|---|---|
| `landuse=forest` / `natural=wood` | `0x2B` |
| `landuse=grass` / `meadow` | `0x38` |
| `landuse=cemetery` | `0x39` |
| `landuse=commercial` / `industrial` | `0x3A` |
| `natural=water` / `water=lake` / closed water area | `0x48` |
| `landuse=residential` | `0x9C` |

**Ring storage:** OSM repeats the first node at the end; Bosch stores each vertex **once** (open loop).
Drop the closing node before computing `count` and emitting the point pool (writer_guide §8).

### 4.4 Names & text records

`name` + `name:*` → a text record in the block, referenced by a `0x7A` name annotation whose payload is the
record's word offset. Multi-language names become the multi-string text record (`read_text_record` inverse:
`{u8 n, u16 len_i…, bytes…}` — verify exact layout in M3).

### 4.5 `state` and feature high byte

- `state` (u16, first cell word): `map2osm_rs` surfaces it as `tm:state` and the dataset uses a near-constant
  value (profile marker). v1: **copy the dominant `state` value from a reference block of the same kind**
  (measure in M0/M1) or write a fixed constant; confirm the reader is indifferent.
- Feature **high byte** = display scale / min-zoom. Set per the level-selection policy (§5). Calibrate in M4.

---

## 5. Level-selection strategy ("selectively choose data for all levels")

An object is emitted at every level `>=` its minimum display level; the feature high byte encodes that
level so the renderer shows it at the right zoom. Policy (v1, tunable):

- **Roads by class:** motorway/trunk → L1; primary/secondary → L2; tertiary/unclassified/residential/service
  → L3 only. (netclass → min level: `0–1→1`, `2–3→2`, `4–7→3`.)
- **Cities by importance:** capital/large city (population or `place=city`) → L1; town → L2; village/hamlet
  → L3. City size class (the `0x21` annotation) derived from population/admin level.
- **POIs by prominence:** fuel / hospital / railway station → L2; parking / shop / amenity → L3.
- **Areas:** forests & water → L2 and L3; urban blocks (`0x9C`) and small landuse → L3 only.

Implementation: a pure function `select(obj) -> Vec<(level, feature)>` driven by the tables above + a few
numeric thresholds (population, highway rank). Keep it data-driven so tuning is a table edit, not code.

Geometry per level: v1 re-encodes the **same** vertices at every selected level (no simplification); optional
Douglas–Peucker for coarse levels is a later optimization.

---

## 6. Tiling & encoding

1. Load region BBox (PAU) from the existing `N6E2AA.IDX` (decompressed). 
2. For each level, build the `5^(2i)` grid; compute each tile's extent + center.
3. Assign each selected object to the tile(s) it intersects (centroid for points; ring bbox/intersection for
   ways/areas). A way crossing a tile edge is **split into per-tile segments**, each its own cell.
4. Per tile, per profile: build the point pool (`i16` deltas `= (coord - center) >> SHIFTS[level]`, deduped),
   the cells (list 0/1/2), and the annotations + text; assemble the block; record the slot
   `{regProf, lenWords, offset}` in the level's tile table.
5. Emit the IDX (header + partition table + tile tables [+ multi sub-lists if ever used]) and each profile's
   MAP (header + contiguous blocks).

---

## 7. Architecture — `src/osm2map_rs`

New crate mirroring `map2osm_rs` (deps: `quick-xml`, `serde`). Modules:

- `osm.rs` — stream-parse the XML into an in-memory model (`Node{id,lat,lon,tags}`, `Way{ids,tags}`,
  `Relation`); resolve way node order; close multipolygon rings. Store coordinates as **i32 PAU ints** (not
  strings) for memory.
- `select.rs` — §4 mapping tables + §5 level policy → per-object `(level, kind, feature, state, name, anns)`.
- `tile.rs` — grid math, tile assignment, cross-tile splitting, delta encoding, per-tile point dedup.
- `emit_map.rs` — assemble blocks (marker/sections/cells/points/annotations/text) per profile; write file.
- `emit_idx.rs` — header + partition table + tile tables; write file.
- `main.rs` — CLI.

CLI (mirrors the sibling tools):

```
osm2map_rs <in.osm> -r N6E2 [-l 123] [-o OUTDIR] [--bbox W,S,E,N]
            → writes decompressed N6E2AA.IDX + N6E2<prof>.MAP to OUTDIR
# then:
cprnav_compress_rs <OUTDIR>/N6E2AA.IDX  <deploy>/N6E2AA.IDX   (per file)
```

Memory: process per tile; never hold the whole region as formatted strings. The largest reference profile is
160 MB decompressed, so streaming + integer coords matter.

---

## 8. Verification strategy

Primary automated gate is a **round-trip**, exactly mirroring `writer_guide.md` §5:

1. `osm2map_rs in.osm → decompressed .IDX/.MAP` → `map2osm_rs <outIDX> -l 0123 → OSM'`.
   Compare `OSM'` to the input: object counts per kind, coordinate accuracy (within the `i16<<shift`
   quantization), and surviving tags/categories. Expect high fidelity on the mapped classes.
2. **BBox containment:** every decoded point inside its tile extent (expect zero violations).
3. **Compress/decompress identity:** `cprnav_decompress(cprnav_compress(osm2map output))` must be
   byte-identical to the `osm2map` output — proves the compressor accepts our file and the container is well-formed.
4. **Byte-diff copied fields:** any header/metadata we copy from a reference must match exactly.
5. **Landmark spot-checks:** Kraków centre, the Vistula river line, a known POI land at real coordinates.
6. **Runtime load (when a car/debug runtime is available):** drop the compressed files in, confirm no load
   error and correct rendering — the ultimate acceptance test.

---

## 9. Milestones / execution order

- **M0 — de-risk (mostly done).** ✅ full read pipeline on real N6E2; ✅ decompressed IDX+MAP headers;
  ✅ profile→filename encoding (`0x8400|prof`); ✅ TCI is optional. Remaining: (a) confirm a single-profile
  region + omitted TCI still loads in the runtime; (b) measure the dominant `state` value; (c) confirm
  `CONTENT.DAT` doesn't encode base sizes (signature.md §7.4).
- **M1 — skeleton that round-trips.** `osm2map_rs` parses OSM and emits a **minimal** valid region (one tile,
  one profile, a handful of points + one line) that `map2osm_rs` reads back without error. Proves the format.
- **M2 — full tiling.** all three levels, point pool + delta encoding, cross-tile splitting; round-trip the
  Kraków box from `krzeszowice.osm` with correct geometry.
- **M3 — semantics.** §4 mapping tables (POI/road/area/netclass) + names/text records; verify categories and
  names survive the round-trip.
- **M4 — level policy.** implement + tune §5 selection; confirm the right objects appear at the right zoom.
- **M5 — compression integration.** wire `cprnav_compress_rs`; assert compress/decompress byte-identity;
  produce deployable CPRNAV_2 files.
- **M6 — full region.** convert all of `krzeszowice.osm` → N6E2; end-to-end round-trip; check size + runtime
  performance.
- **M7 (later).** augment-existing-map mode; RNW routing writer; on-car render test.

---

## 10. Open questions / risks

| # | question | plan / mitigation |
|---|----------|-------------------|
| 1 | Does the runtime require the original **5-profile** set, or is a single profile fine? | M0: try single-profile region in the runtime; fall back to emitting all 5 (split objects across them) if needed. |
| 2 | Is omitting `.TCI` tolerated for rendering? | M0: load without TCI; else emit a copied/minimal TCI per profile. |
| 3 | Correct `state` value per object kind? | M0/M1: measure the dominant value from reference blocks; confirm reader indifference via round-trip. |
| 4 | Feature **high byte** (display scale) exact semantics? | M4: calibrate against how `map2osm_rs`/renderer treat it; start with level index. |
| 5 | Does `CONTENT.DAT` (signed) encode base file sizes? | signature.md §7.4 — if yes, keep output sizes stable or accept that region is out of reach for a signed swap. |
| 6 | Text-record exact byte layout for multi-name records? | M3: reverse `read_text_record` precisely on a reference block before emitting names. |
| 7 | Cross-tile way splitting correctness (shared boundary vertices). | M2: dedup points per tile; verify no gaps/dupes at edges in the round-trip. |
| 8 | Coordinate quantization overflow (`i16 << shift`). | clamp/split objects whose span exceeds a tile's delta range; assert in emit. |

---

## 11. Reference artifacts

- Verified decompressed N6E2 files: `/tmp/opencode/rt/` (`N6E2AA.IDX`, `N6E2{102,10E,10H,10I,11A}.MAP`).
- `src/map2osm_rs/src/main.rs` — the reader; **inversion source** for all §4 tables.
- `src/cprnav_compress_rs`, `src/cprnav_decompress_rs` — container codec (both verified).
- `doc/TravelMap_format/03 - writer guide/writer_guide.md` — byte-level write recipe + pitfall checklist.
- `doc/TravelMap_format/02 - details/MAP_format.md` — full read-path format reference.
- Input: `/home/marek/Ext/reverse_engineering/NissanMaps/OSM-map/krzeszowice.osm`.
