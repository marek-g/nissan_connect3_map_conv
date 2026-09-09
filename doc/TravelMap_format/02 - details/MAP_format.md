# Navigation Map Data Format — Bosch TravelMap / Nissan LCN2KAI (EUR 2021.Q1)

Complete documentation of the `.IDX` / `.MAP` / `MAPWORLD.MAP` file format, reverse
engineered from the `DAPIAPP.OUT` binary (Ghidra, project "Nissan Ghidra Project") and
verified against the firmware data.

Converter implementing this format: [`map2osm_rs/`](../../../src/map2osm_rs/) (OSM XML, Rust;
build with `cargo build --release`). Road-name enrichment: `rnw_extract_rs` +
`rnw_join_rs` (also OSM XML in/out).

---

## 1. File Overview

Source directory: `CRYPTNAV/DATA/DATA/MAP/` (411 regions, ~907 MB).

| File | Role |
|------|------|
| `<REGION>AA.IDX` | One per region. Tile tables (4 levels) pointing to blocks inside the MAP files. |
| `<REGION>1XX.MAP` | Eight per region (one per `regProf`). Contain the actual geometric data. |
| `<REGION>1XX.TCI` | Tile Cluster Index per MAP file — auxiliary "cluster" tile index. Not needed for the `.IDX`→`.MAP` geometry path; empty for profile 10I. See §11. |
| `MAPWORLD.MAP` | Global world partition (region/tile grid); 4 rows of 8 B starting at offset 0x28. |

The directory `CRYPTNAV/DATA/CONNECT/MAP/` also holds copies of the `.IDX` files plus
`IDX_CNT.TBL` (identical contents).

### Region name encoding

Region = `N<row>E<column>`, e.g. `N6E2`. Rows are latitude bands, columns are longitude
bands (band width depends on the row — see §3).

MAP file name for a given profile:

```
<REGION> + "1" + base32(regProf & 0xFF)      # base32 = "0123456789ABCDEFGHIJKLMNOPQRSTUV"
e.g. regProf=0x040b -> N5E2 + "1" + "0B" = N5E210B.MAP
```

### Example: Poland

Poland (≈ 14–24°E, 49–55°N) lies within two regions:

| Region | BBox (degrees) |
|--------|----------------|
| `N6E1` | [0, 18] × [47.25, 56.70] |
| `N6E2` | [18, 36] × [47.25, 56.70] |

---

## 2. Coordinate Unit (PAU)

Coordinates are stored in **PAU** (Private Angular Unit):

```
deg = pau * 180 / 2^31          (pau = deg * 2^31 / 180)
```

Coordinates in the files are **deltas** relative to the tile center, scaled by the
level `shift` (see §3). The delta is multiplied by `2^shift` and added to the tile's
BBox center.

---

## 3. Global World Partition — `MAPWORLD.MAP`

Loaded by:

- **`dap_map_tclWorldTilePartition::u16Load` @ `0x008edf7c`**
  - path: `<devpath>/data/data/map/mapWORLD.map` (case-insensitive comparison)
  - `bSkipBuffer(0x28)` — skips the 40-byte header
  - then 4× (levels L0..L3): `bReadU8(skip_flag)`, `bReadU8 -> this+4+i` (longPart),
    `bReadU8 -> this+8+i` (latPart), `bReadU8 -> this+0xC+i` (shift), `bSkipU32` (tileCnt)
  - on success sets the flag `this+0x40 = 1`

Structure from offset **0x28** (4 rows × 8 B):

| offset | field | value (EUR 2021.Q1) |
|--------|-------|---------------------|
| +0 | `skip_u8` (level index) | 0,1,2,3 |
| +1 | `longPart` | **1, 5, 10, 10** |
| +2 | `latPart`  | **1, 5, 10, 10** |
| +3 | `shift`    | **13, 10, 7, 4** |
| +4..+7 | `u32 tileCnt` | **1, 25, 2500, 250000** |

Meaning:

- `shift[i]` — coordinate-delta shift at level `i`: `out = center + delta << shift[i]`.
- `longPart[i]` / `latPart[i]` — X/Y axis division used by the hierarchical tile numbering.
- `tileCnt[i]` — number of tiles of level `i` in a region (product of `latPart[0..i]`).

> **Note:** earlier reads of this table were off by 1 byte (a bug). The correct values
> are those in the table above; `shift = (13,10,7,4)`.

### Region grid (bands)

Rows (`N1..N8`) are bands **9.45°** wide (0 … 75.6°N). Column band width depends on the
row:

| Row | Column width (°E) |
|-----|-------------------|
| N1, N2 | 10 |
| N3 | 11.25 |
| N4 | 12 |
| N5 | 15 |
| N6 | 18 |
| N7, N8 | 25.71 |

Each region's BBox is stored directly in its `.IDX` header (§4), so in practice the grid
does not have to be reconstructed — just read `west/south/east/north`.

---

## 4. The `.IDX` File (per region)

Loaded by:

- **`dap_map_tclTileLoader::u16LoadIdxStructures` @ `0x008e352c`**
  - file name from `u16GetIndexFilename(desc, buf, "idx")`
  - header: `dap_map_tclIdxFileHeader::bRead(this, ...)`
  - partition table offset = `*(u16*)(header + 0x14) * 4`
  - 4× `dap_map_tclMapPartition::bRead(this + 0xEC + i*0xC, ...)` — 12 B each

### Header (32 B)

| offset | type | field |
|--------|------|-------|
| 0x00 | u16 | `binOff` — binary section offset (= start of the L0 table) |
| 0x02 | u16 | (spare; == 32 in observed files) |
| 0x04 | u32 | `west`  (PAU) |
| 0x08 | u32 | `south` (PAU) |
| 0x0C | u32 | `east`  (PAU) |
| 0x10 | u32 | `north` (PAU) |
| 0x14 | u16 | partition table offset (in 4-byte units) |

### Partition table (4 × 12 B, from `binOff_part = u16@0x14 * 4`)

| byte | field |
|------|-------|
| +0 | `i` — level index (0..3) |
| +1 | `latPart[i]` |
| +2 | `shift[i]` |
| +3..+6 | `u32a = (tileCnt << 8) \| (13 - 3*level)` |
| +7..+10 | `u32b = (tableOffset << 8)` — offset of this level's tile table |

Tile counts: **L0=1, L1=25, L2=2500, L3=250000** (all regions).
Tables: L0 @ `binOff`, L1/L2/L3 @ `u32b >> 8`.

### Tile table — slot (8 B per tile K)

```
u16 regProf : bit15 = empty (0x8000), bit14 = multi (0x4000), low 14 bits = profile
u16 length  : block length in the MAP file, in 4-byte words (the block marker stores this)
u32 offset  : block offset inside the .MAP file
```

- `empty` — no data for this tile.
- `multi` — the slot is a pointer to a **sub-entry** list: `{count(≤15), ptr}`, where
  `ptr` points (within the same IDX) at `count` consecutive 8-byte slots (the real entries).
- profile → MAP file name (§1).

---

## 5. The `.MAP` File — Data Block

### MAP file header (32 B)

Verified on all 8 profiles of N6E1/N6E2 (and consistent across regions):

| offset | type | field |
|--------|------|-------|
| 0x00 | u16 | binary section offset — first data block in the file (e.g. 0x7b4…0x7c4) |
| 0x02 | u16 | info-string table offset (== 52 = 0x34 in all observed files) |
| 0x04 | u32 | total file size (in bytes) |
| 0x08 | s32 | `west` (PAU) — region BBox, identical to the IDX header |
| 0x0C | s32 | `south` (PAU) |
| 0x10 | s32 | `east` (PAU) |
| 0x14 | s32 | `north` (PAU) |
| 0x18..0x1F | 4×u16 | not decoded (observed: 8, 4, 0x404, 0x8412); not needed for geometry |

At offset 52: an array of `u16` offsets pointing at ASCII info strings
(`"Copyright by Robert Bosch GmbH Hildesheim 2021"`, build date,
`"TpMap2 (Map-Data) for TravelMap"`, `"Project : 2-LCN2KAI_NT_INT_EUR_2"`, …).
This is **not** a text/name section — feature names live inside the blocks (§8).

The binary section (from `u16@0`) contains the data blocks; block offsets from the IDX
are absolute file offsets and span exactly `[u16@0, fileSize)`.

### Block header

| offset | type | field |
|--------|------|-------|
| +0x00 | u32 | `marker = 0xFFFF \| (length << 16)` |
| +0x04 | u16 | `start[0]` — start of list 0 (polygons), in 4-byte units |
| +0x06 | u16 | `count[0]` — number of cells in list 0 |
| +0x08 | u16 | `start[1]` — start of list 1 (lines) |
| +0x0A | u16 | `count[1]` |
| +0x0C | u16 | `start[2]` — start of list 2 (POI) |
| +0x0E | u16 | `count[2]` |

Cell regions **chain**: `start[i+1] = start[i] + count[i]*3`
(one cell = 12 B = 3 units of 4 B). After them: the point pool, then the annotation /
text section (§8).

### Cell (always 12 B)

**Lists 0 and 1 (polygons / lines):**

| offset | type | field |
|--------|------|-------|
| +0 | u16 | `state` |
| +2 | u16 | `feature` — `{displayScale:8 \| featureCode:8}` |
| +4 | u16 | `pointIdx` — start index in the point pool (in 4-byte units) |
| +6 | u16 | `count` — number of points |
| +8 | u32 | `annotDesc = {u16 startUnit, u16 count}` — annotation list (§8) |

Points in the pool @ `pointIdx*4`: `count × {s16 dlon, s16 dlat}`.

**List 2 (POI):**

| offset | type | field |
|--------|------|-------|
| +0 | u16 | `state` |
| +2 | u16 | `feature` |
| +4 | s16 | `dlon` — delta stored directly in the cell |
| +6 | s16 | `dlat` |
| +8 | u16 | `annotDesc.startUnit` (§8) |
| +10 | u16 | `annotDesc.count` |

**Premium POI:** when `feature & 0xF000 == 0xF000` (list 2) — no plain coordinates; the
8-byte payload is skipped. Handled by **`bSkipPremiumPOI` @ `0x008d5f00`** (only for
`listType==2`). This case does not occur in the N6E1/N6E2 data.

### Read path — confirmed cell layout

The read loop is **`u16ConvertCells` @ `0x008d7660`**. For each cell it reads:
`state (u16)`, `feature (u16)`, then either skips 8 bytes (premium POI) or dispatches to
**`u16WriteData` @ `0x008d760c`**, which routes by list type:

- listType 0/1 → **`u16WritePolyLineData` @ `0x008d73c8`** (reads `pointIdx`, `count`, then the annotation descriptor)
- listType 2   → **`u16WritePOIData` @ `0x008d6c24`** (reads `dlon`, `dlat`, then 2× `u16` = the annotation descriptor; it is pushed onto a `TextIdWritePosStack` and handed to the annotation converter, §8)

This confirms the 12-byte cell layout above: `{state, feature}` + 8 bytes of type-specific
payload, where the trailing u32 in **all** lists is the annotation list descriptor.

### Chaining and conversion — Ghidra functions

| Function | Address | Role |
|----------|---------|------|
| `u16Convert` | `0x008d8248` | block conversion entry (MemBlockDesc + DataAccess) |
| `u16ConvertList` | `0x008d7a0c` | convert one list; `bSetRelPos(start*4)` |
| `u16ConvertCells` | `0x008d7660` | loop over the list's cells |
| `u16WriteData` | `0x008d760c` | per-list-type dispatch of the 8-byte payload |
| `u16WritePolyLineData` | `0x008d73c8` | polygon/line (lists 0/1) |
| `u16ConvertCoordList` | `0x008d67e8` | convert a coordinate list from the pool |
| `u16ConvertCoords` | `0x008d66ac` | single pair: `out = center + (delta << shift)` (asm: `add r6,r2,r12,lsl r0`) |
| `dap_map_tclListDesc::bRead` | `0x008d973c` | read the 4-byte descriptor `{pointIdx, count}` |
| `u16WritePOIData` | `0x008d6c24` | POI (list 2) |
| `bSkipPremiumPOI` | `0x008d5f00` | skip premium POI |

---

## 6. Tile Numbering and Tile BBox

The tile number `K` of level `i` decomposes hierarchically via
**`vCalcLvlBasedTileId` @ `0x008c970c`**:

```
L3: p4 = K//10000 ; p5 = (K//100)%100 ; p6 = K%100
    col = (p4%5)*100 + (p5%10)*10 + (p6%10)
    row = (p4//5)*100 + (p5//10)*10 + (p6//10)     # 500x500 grid
L2: p4 = K//100 ; p5 = K%100
    col = (p4%5)*10 + (p5%10)
    row = (p4//5)*10 + (p5//10)                    # 50x50 grid
L1: col = K%5 ; row = K//5                          # 5x5 grid
L0: whole region
```

(The Ghidra implementation does this via an L3→L2 fall-through with `r7 = K/100`, and
`p4 = K & 0xFF` for levels < 2.)

### Tile BBox

Border functions:

| Function | Address | Role |
|----------|---------|------|
| `u16GetRelLBorder` | `0x008cab54` | left border: sum `Σ w_i * col_i`, `col_i = (m_i-1) % longPart(i)` |
| `u16GetRelLowerBorder` | `0x008ca69c` | lower border: sum `Σ h_i * row_i`, `row_i = (m_i-1) / latPart(i)` |
| `u16GetRelUpperBorder` | `0x008ca3fc` | upper border (recursion for the last row; result = sum with `row+1`) |
| `u16ShrinkBBoxByTile` | `0x008cac6c` | align/shrink the BBox to the tile |
| `u16CalcBBoxOfUniqueId` | `0x008caf5c` (variant `0x008475ac`) | compute BBox from a unique ID |

Closed-form formula (verified, see §9):

```
W = east - west ; H = north - south
L0: box = [west, south, east, north]
L1: c=K%5, r=K//5        -> [W*c/5,   H*r/5,   W*(c+1)/5, H*(r+1)/5]  (relative to west/south)
L2: col,row per formula  -> [W*col/50, H*row/50, W*(col+1)/50, H*(row+1)/50]
L3: col,row per formula  -> [W*col/500, H*row/500, W*(col+1)/500, H*(row+1)/500]
```

Then **align every edge down to `2^(shift+1)`** (in PAU), and the tile `center` = the
midpoint of the aligned box. That `center` is the base for the coordinate deltas.

> **Historical bug:** the upper border is `south + rel_n`, NOT `north + rel_n`.

---

## 7. Feature → Type Classification

Dispatch: **`u8ConvertFeature2Type` @ `0x008d5b8c`** — takes `feature & 0xFF`
(the low byte) and `listType`:

| listType | name | function | address |
|----------|------|----------|---------|
| 0 | polygons | `u8ConvertFeature2PolyType` | `0x008d56a0` |
| 1 | lines | `u8ConvertFeature2LineType` | `0x008d582c` |
| 2 | POI | `u8ConvertFeature2POIType` | `0x008d5a10` |

**Feature high byte = POLYGON display-scale selector ONLY (this is NOT the #06c reboot cause).**
Ghidra-verified data path: `u16ConvertCells@0x8d7660` reads `[state:u16][feature:u16]`, masks
`feature &= 0xff` for the type converters, and passes **`feature>>8` (the high byte)** down through
`u16WriteData@0x8d760c` → `u16WritePolyLineData@0x8d73c8` → `u16Convert2TypeDisplayScale@0x8d68d8`, which
branches by convTypeTag: **tag 0 (polygon) → `u16Convert2PolyDisplayScale(high)` →
`PolygonConfigMatrix::u16GetDisplayScale@0x8d9b08`**, which scans 14 runtime thresholds and returns
`s_au16DisplayScale[0..0xd]` — **clamped, so high `0x00` is NOT an OOB; it only selects a wrong (top) scale
bucket**. **tag 1/2 (line/POI) → `u16Convert2LinePOIDisplayScale(this)` which IGNORES the feature entirely**
— that is why our roads and POIs render fine no matter what high byte we emit. Measured over 178k stock
polygon cells: **polygon high byte spans `0x20..0x95`, line/POI high byte is always `0x00`.**

Consequence: the whole DAPIAPP converter is defensive (`bCheck` before every read, clamped table indices →
malformed data returns error codes `0xd`/`0x218`/`0x321`, never faults), so **neither the high byte nor a
bad coord-list count can reboot the unit inside DAPIAPP.** The #06c reboot (roads rendered, adding our
polygons rebooted) is therefore **still UNCONFIRMED.** `osm2map` still maps each area low code to a
stock-canonical FULL code (`0x9c→0x209c`, `0x38→0x6038`, `0x2b→0x602b`, `0x48→0x5048`, `0x39→0x4039`,
`0x3a→0x503a`; unknown low → high `0x40`) so polygons land in the normal display-scale range (fidelity
only — not a proven #06c fix).

**Renderer (procmapengine) polygon path reviewed — every branch is bounded/guarded for OUR data:**
- `LandUseAreaAddToDrawLayers@0x3ef7d0` indexes a style array by `this+0x58` but ONLY skips when it is the
  `0xffff` sentinel; `GetPolyConfigOffsetBasedOnType@0x46cd08` maps (type,subtype[,sub2]) → offset `0..0x35`
  (bounded, `0xffff` for unknown types) → `param_3+100 + off*4`. Landuse types we emit resolve to valid
  offsets.
- `map_tclTriangulate::u16TessellatePolygon@0x3b8df0` (ear-clip) stores ring vertex indices in **one BYTE**
  and fills its list with `do{ idx[i]=..; i=(i+1)&0xff } while(i<n)` ⇒ **for `n ≥ 256` that loop never
  reaches `n` → INFINITE LOOP → watchdog reboot**. This is a genuine Bosch bug, BUT it is **inactive for our
  data**: per-tile `clip_polygon` already bounds every emitted ring (measured max **198** vertices, pre- AND
  post-fix; 0 rings ≥256), so it is NOT what rebooted #06c. osm2map adds `decimate_ring` (closed
  Douglas-Peucker, cap 250, `MAX_RING_PTS`) as a **preventive** guard so this never bites if grid/OSM change.
- Tessellation *failure* (bad/self-intersecting ring, `n<256`) is handled → `ReadFrom` frees buffers, returns
  1 gracefully; no fault.

⇒ The offline model is exhausted again: loader, DAPIAPP converter, and the renderer landuse/tessellation path
all tolerate our (valid, ≤198-vertex) polygons. **Decisive next step is on-device**: capture the #06c crash
trace (`diag/navdiag_logger.sh`) to get the faulting module/PC instead of guessing across ~13k renderer
functions.

### Lines (list 1) — code → type

| code (hex) | type |
|------------|------|
| 0x07 | 100 |
| 0x10–0x17 | 1 |
| 0x20 | 3 |
| 0x21 | 2 |
| 0x30–0x37 | 4 |
| 0x71–0x73 | 100 |

> **Water lines vs road lines (writer-critical).** A line's low code selects its reader: `0x20`
> (type 3) = **hydrography** line → consumes the `0x10` water annotation; `0x30..0x37`/`0x21`
> (types 4/2) = **road** line → its reader expects the `0x11` roadinfo. So a water line MUST use
> feature `0x20`, never `0x30`. Emitting `0x10` on a `0x30` line makes the road reader misparse the
> annotation and reboots the head unit (confirmed on car, trial #09l vs #09n). Stock N6E2: water
> lines `0x20`.
>
> **Road-class tiers (writer-critical — why roads looked identical, and why residential looked like a
> major).** Within the road lines the feature **low byte encodes the road class tier**, and the base-map
> renderer styles the pen (weight + colour) by it. STOCK-MEASURED via `map2osm` over the N6E2 Kraków
> bbox (L2 ways that carry a `0x11` roadinfo → netclass), the map is **one code per arterial class**:
> `0x30` motorway (`nc0`) · `0x31` trunk (`nc1`) · `0x32` primary (`nc2`) · `0x33` secondary (`nc3`) ·
> `0x21` **everything local** (`nc≥4`: tertiary/unclassified/residential/living_street/service/track/path).
> The classed arterials `0x30..0x33` carry a `0x11` roadinfo (netclass + link/roundabout/toll); the
> `0x21` local network carries **no `0x11`** and dominates the finest level (stock **L3 has only `0x21`
> lines**, no `0x30..0x37`). `u8ConvertFeature2LineType` collapses `0x30..0x37`→type 4 and `0x21`→type 2.
> Two writer traps: (1) one code for every road draws motorways and footpaths with the same pen; (2)
> getting the tier mapping wrong is as bad — `osm2map` once emitted `nc4-6→0x33`, which is the yellow
> *secondary* pen, so residential streets rendered as major roads (trial #13 visual bug; fixed in #14 to
> the table above). `osm2map` maps OSM `highway` → netclass via `roadinfo_w()`, then netclass → tier via
> `road_feat_low()`. See `trials/13` (codes boot OK) and `trials/14` (stock-correct tiers).


### POI (list 2) — code → type

| code (hex) | type |
|------------|------|
| 0x01–0x09 | 1–9 |
| 0x10–0x17 | 0xA–0x11 |
| 0x20–0x25 | 0x12–0x17 |
| 0x26 | 0x1B |
| 0x27 | 0x1C |

**Verified POI category (low byte)** — cross-checked against object `name`s in a Kraków L3
extract and against the official icon taxonomy in `POI_MAPPING.DAT` (see §7.1):

| feature (low byte) | category | evidence (Kraków names / amenity) |
|--------------------|----------|-----------------------------------|
| 0x01 | settlement / city (place) | 1890 stock city POIs all `feat=0x01` + a `0x21` annotation (§8) |
| 0x02 | parking | "P+R", amenity=parking |
| 0x04 | fuel / petrol station | CIRCLE K, PETRONET, AUCHAN; amenity=fuel |
| 0x05 | hotel | HOTEL LORENZO, PLATINUM, HOTEL PROKOCIM |
| 0x06 | restaurant / food & drink | KAPITAN, PRZYSMAK (amenity=restaurant subset) |
| 0x07 | car service / motor | CENTRUM MOTOCYKLOWE, SPEED CAR, auto |
| 0x08 | company / office | "…SP. Z O.O." (few) |
| 0x09 | car rental | SIXT, BLUE RENTAL, 99RENT |
| 0x10 (16) | school / education | PRZEDSZKOLE, ZESPÓŁ SZKÓŁ, AWF |
| 0x11 (17) | bar / pub / nightlife | LOKOPUB, MIESZCZANSKI |
| 0x12 (18) | sport / fitness / recreation | KRYTA PŁYWALNIA, FITNESS |
| 0x13 (19) | pharmacy / hospital | APTEKA, SZPITAL, BONA |
| 0x14 (20) | shop / supermarket / retail | BIEDRONKA, SHELL, CROSBIKE |
| 0x15 (21) | bank / ATM | CASHZONE, EURONET, BANK SPÓŁDZIELCZY |
| 0x16 (22) | church / cemetery | MB RÓŻAŃCOWEJ, CMENTARZ |
| 0x17 (23) | park / green space | PARK OSIEDLOWY, LASEK (+ some church) |
| 0x22 (34) | railway station | KRAKÓW PROKOCIM, KRAKÓW BIEŻANÓW |
| 0x23 (35) | highway junction / interchange | WĘZEŁ BIEŻANÓW |

### Polygons (list 0) — code → type

| code (hex) | type |
|------------|------|
| 0x07, 0x0F | 100 |
| 0x10–0x17 | 1 |
| 0x18–0x1F | 0x20 |
| 0x20–0x24 | 3 |
| 0x28–0x2C | 0x22 |
| 0x30–0x32 | 2 |
| 0x38–0x3A | 0x21 |
| 0x40 | 5 |
| 0x41 | 6 |
| 0x48 | 0x24 |
| 0x49 | 0x25 |
| 0x50–0x57 | 0x65 |
| 0x58–0x5F | 0x33 |
| 0x60–0x64 | 0x67 |
| 0x68–0x6C | 0x35 |
| 0x70–0x72 | 0x66 |
| 0x78–0x7A | 0x34 |
| 0x80 | 0x67 |
| 0x88 | 0x35 |
| 0x94 | 4 |
| 0x9C | 0x23 |

 **Verified land-use class (polygon low byte)** — the low byte picks the terrain class, which is
 what the renderer colours (high byte measured `0x00` throughout — see §7 note above). From a Kraków-area L3 extract
 (city + suburbs + rural south), with the OSM tag now emitted by the converter:

 | feature (low byte) | terrain | evidence (names / size / location) | OSM tag | map colour |
 |--------------------|---------|-------------------------------------|---------|-----------|
 | 0x9C | urban blocks (dominant) | 4279 small (~0.04 km²) unnamed, city-centre centroid | `landuse=residential`* | light grey |
 | 0x38 | rural open land | 737 large (~5.7 km²) unnamed, rural south | `landuse=grass`* | green |
 | 0x2B | forest / woodland | LAS BRONACZOWA, PUSZCZA NIEPOŁOMICKA (large, south) | `natural=wood` + `landuse=forest` | dark green |
 | 0x39 | cemetery | CMENTARZ GORZKÓW, CMENTARZ KOMUNALNY, … | `landuse=cemetery` | grey-green |
 | 0x3A | commercial / shopping | DEKADA, PARK HANDLOWY ZAKOPIANKA, BONARKA CITY CENTER (+ some hospital) | `landuse=commercial`* | brown/grey |
 | 0x48 | water body | WISŁA, STAW PŁASZOWSKI, ZALEW BAGRY | `natural=water` + `water=lake` | blue |

 \* best-effort: these classes are unnamed and mixed (urban blocks can be residential/commercial/
 industrial; rural open land can be grass/meadow/farmland; 0x3A mixes shopping with some hospitals).
 The exact code is preserved in `tm:feature` for a finer pass. So the "grey = urban, green =
 forest/grass, blue = water" scheme the user expects is driven by these low-byte classes; the exact
 RGB per class lives in the renderer's style config (see §7.2).

**`tm:state` (cell byte 0–1)** — a constant per dataset/profile, **not** a category: every
feature in an N6E2 L3 extract carried `state = 16876 (0x41EC)` regardless of kind or feature.
It is emitted for completeness but carries no per-object discrimination at this level; treat it
as a profile/variant marker.

> The numeric `type` codes are the internal FastMap type. The human-readable layer name
> in the converter is derived from **kind + annotation categories** (section 8), which is
> what the data itself supports:

### Derived `layer` property

| kind | condition | layer |
|------|-----------|-------|
| poi | has 0x21 city annotation | `poi:city` |
| poi | has 0x30 fuel | `poi:gas` |
| poi | has 0x23 parking | `poi:parking` |
| poi | has 0x34 restaurant | `poi:restaurant` |
| poi | has 0x22 rest area | `poi:rest_area` |
| poi | has 0x35 brand chain | `poi:brand` |
| poi | otherwise | `poi` |
| line | has 0x14/0x11 road annotation | `road` |
| line | has 0x10 water annotation | `water` (rivers, canals) |
| line | otherwise | `line` |
| polygon | has 0x10 water annotation | `water_area` |
| polygon | otherwise | `area` |

(POI categories are joined with `+` when several apply. Subtype codes from
`u8ConvertFeature2{Poly,Line}SubType` @0x008d5bb8/0x008d58a4 exist but map to further
internal numbers; full semantic names like "forest"/"highway" live on the rendering
side and are not in the MAP data.)

### 7.1 POI icons — `POI_MAPPING.DAT` (SQLite)

The fine-grained POI category → icon mapping is **not** in the `.MAP` file; it ships as a
SQLite database at `CRYPTNAV/CFG/LID/POI_SRV/POI_MAPPING.DAT`. The MAP cell's small feature
code (above) is the *coarse* class; the specific brand/type (and therefore the icon) comes from
the POI's annotation and is resolved through this DB. Key tables:

> **Code-space note.** `POIMappingTab.FeatCode` is **not** the MAP cell feature byte. It is a
> POI-**service/search** category id in the `7310..9996` (`0x1C8E..0x270C`) range (e.g. `PETROLSTATION`
> = `0x1C8F`, `CARRENTAL` = `0x1C90`, `HOTELMOTEL` = `0x1C92`), used by the search/POI-server index — a
> different taxonomy from the render low byte (`0x04` fuel, `0x05` hotel, …). This DB supplies the
> human **category tree** (`IndexCat`/`MapGroup`/`MapSubGroup`) and **brand→icon** mapping, not a direct
> render-byte lookup.

- **`neh_IDTable(name, Enum, ID)`** — 209 rows: icon file name → `FI_EN_*` category enum → numeric id.
  This is the icon catalogue (129 distinct `.png`, 202 enums), e.g.:

  | icon file | enum | id | meaning |
  |-----------|------|----|---------|
  | FUEL.png | FI_EN_GASSTATION | 221 | petrol station |
  | PARKING.png | FI_EN_PARKING / _PARKINGGARAGE / _OPENPARKINGAREA / _PARKANDRIDEFACILITY | 30/902/286/905 | parking (variants) |
  | COFFEE_SHOPS.png | FI_EN_CAFE / FI_EN_COFFESHOP | 209/371 | café / coffee shop |
  | RESTAURANT.png | FI_EN_RESTAURANT / _RESTAURANTAREA | 243/285 | restaurant |
  | FASTFOOD.png | FI_EN_FASTFOODRESTAURANT, _F_AMERICANFOOD, _BREAKFAST | 376/4174/907 | fast food |
  | CHINESE/ITALIAN/JAPANESE/… .png | FI_EN_F_*FOOD | … | cuisine sub-types |
  | AUTO.png / CAR_WASHES.png / SERVICE_MAINTENANCE.png | FI_EN_GROUPFUELAUTO / _CARWASHES / _GROUPSERVICEMAINTENANCE | 3001/901/3003 | automotive services |

- **`POIMappingTab(..., MapGroup, MapSubGroup, FeatCode)`** — the full category tree; `MapGroup`
  is an icon-group enum `E_MAP_IMAGE_POIICON_<TOPIC>_GROUP_<NAME>` (e.g.
  `…_PETROL_STATION`, `…_HOTEL`, `…_RENT_A_CAR_FACILITIES`, `…_PHARMACY`, `…_HOSPITAL`,
  `…_ATMCASH_DISPENSOR`, `…_CINEMA`, `…_GOLF_COURSE`). Top-level topics: ACCOMMODATION,
  AUTOMOTIVE, CITY, ENTERTAINMENT_AND_RECREATION, MORE (bank/ATM/business), PUBLIC_FACILITIES
  (school/hospital/pharmacy/post/police/cemetery…), RESTAURANTS, SHOPPING, TOURISM.
- **`br_GMNextGen_POIs_brands(Brand, Icon, IconFile, ChainID, FIID, AnnotType, AnnotValue)`** —
  brand chains (e.g. a specific gas chain) → concrete icon file + the `AnnotType`/`AnnotValue`
  pair that appears in the MAP annotation. This is the bridge from MAP bytes to a named icon.

Note: the referenced icon `.png` files are **not** present in the firmware tree examined (only
other art, e.g. EV-charging `I00x_T*.png`, and 3D instruction pictures under `INSTRUCT/3D_PICT`);
the POI icon package is loaded from elsewhere at runtime. The semantic mapping above is what
matters for identifying a POI's meaning; the pixel art is looked up by these names.

### 7.2 Rendering & colour — how `feature` becomes a drawn colour/icon

`procmapengine.out` (Bosch `rbin_fastmap_3d_13.1 …/mapengine`, GLES + SVG) is the renderer. It
does **not** hard-code per-feature colours; it resolves them through a style config:

1. The MAP cell's feature is decoded by `map_tclFastMapToMapFeature::FastMapToPolyMapFeature`
   (lines/areas) into a typed map feature.
2. `map_tclMapEngineConfig::GetPolyConfigOffsetBasedOnType(config, feature)` maps that feature
   type → a **style index**.
3. The style index indexes a per-type config table in `map_tclMapEngineConfig` (an array of
   pointers at `config+100`); each entry holds the fill/border attributes (colour, border on/off,
   etc.). `map_tclMapElm_Landuse_Area::ReadFrom` reads this and builds the vertex/index buffers;
   `Draw`/`PrepareVertices2D` submit them to GLES.

So land-use colour is: **feature low byte → poly type → style index → config entry (RGB)**. The
concrete RGB values are runtime `map_tclMapEngineConfig` globals (`CONF_*`, e.g.
`CONF_BACKGROUND_COLOR_DAY/NIGHT`, `CONF_MAP_HLG_MAPELM_COLOR_GROUND_ID_DAY`, …) with defaults in
the binary, overridable by the device's theme — which is why the same land-use class can be drawn
grey (urban), green (park/forest) or blue (water) depending on the style entry, day/night mode.
POI icons are resolved separately through `TextureManager` / `map_tclCacheElement_Icon` using the
§7.1 catalogue.

---

## 8. Annotations and Text Records (fully decoded)

The `t0`/`t1` pair in **every** cell (poly, line *and* POI) is a list descriptor:

```
t0 = startUnit   u16   first annotation unit index
t1 = count       u16   number of annotations
```

Annotations live at `startUnit * 4` inside the block (after the point pool) and are a
packed sequence, each `{u8 size, u8 type, payload[size-2]}` (`size` counts itself + type).
`count == 0` → no annotations.

> ### ⚠ Residential/settlement polygons (feat low `0x9c`) REQUIRE a `0x04` DCM annotation — or the head unit reboots
>
> **Empirically confirmed on the car (ladder #06c3, see `trials/06 - isolation ladder`):** our
> generated land-use polygons carried `annotDesc = (0,0)`. Rendering them — even 50 in the boot view —
> **hard-faulted and rebooted the head unit**; adding `0x04` made them render. Root cause of the
> #04/#05/#06c/#06c2/#07 reboot loop.
>
> **It is category-specific, NOT a generic "every polygon needs an annotation" rule.** Cross-tab of
> `feature` low byte × first-annotation type (all N6E2 `list0`, marker-validated) shows `0x04` lives on
> **exactly one category** and is *required* there:
>
> | `feature` low | OSM meaning | cells | with `count=0` | first annotation |
> |---|---|---|---|---|
> | **`0x9c`** | **`landuse=residential` (settlement area)** | 2 779 023 | **0 %** | **`0x04` DCM (2 770 696)** |
> | `0x38` | grass / meadow | 347 118 | 79 % | (rest) `0x7A` name |
> | `0x2b` | forest / wood | 194 904 | 96 % | `0x7A` |
> | `0x48` | water (area) | 124 449 | 0 % | `0x7A` + `0x10` (never `0x04`) |
> | `0x39` | cemetery | 59 015 | 81 % | `0x7A` |
>
> So `0x04` (DCM = Digital/3D City Model) is the **settlement-area** record — "this urban block has a 3D
> city model, class N" — which is exactly why stock puts it on `residential` and never on grass/forest/water.
> Grass/forest/cemetery are legitimately *unannotated* 79–96 % of the time and boot fine. The `0x04` record is
> 8 bytes; the validated stock value we emit verbatim is `08 04 | 20 00 | 11 00 | 00 00`
> (`{size=8, type=4, u16=0x0020, u8=0x11, u8=0x00, u8=0x00, u8=0x00}`); per `u16ConvertDCMInfo` the `u16=0x0020`
> low byte is the DCM-class selector (`u8ConvertDCMClass(0x20)=2`), `0x11`/`0x00` are two ×10-scaled params
> (model height/extent, rendering-side), last 2 bytes unused. **`osm2map` emits `0x04` only on `0x9c`**
> (`OSM2MAP_POLY_ANN=cat`, the default; `all` = on every polygon, `0` = none). Water-area `0x48`
> polygons carry the `0x10` water annotation (`{size=4,type=0x10,u16 code}`), stock's dominant lake value
> `type=2/class=8` = `0x28` — `osm2map` emits this by default (`OSM2MAP_WATER_POLY_ANN=0` disables).
>
> Mechanism: `map_tclMapElm_Landuse_Area::ReadFrom @ 0x003ebc64` resolves a style via
> `GetPolyConfigOffsetBasedOnType` and dereferences `configTable[off]` (→ `*(float*)(pmVar13+0x38)`); the
> residential style needs the DCM record, so with `annotDesc=(0,0)` the entry is absent and the deref faults.
> Line/POI cells never take this path (they carry their own annotations). We have not isolated whether *any*
> annotation on `0x9c` suffices vs this specific `0x04` (no serial log); the exact tested bytes are kept.


### Annotation types

Dispatch: `dap_map_tclAnnotationConverter::u16WriteAttrib` @ `0x00920744`.

On-disk payload layouts (verified against N6E1 L2 data + Ghidra; `size` = header byte,
payload = `size - 2` bytes):

| type | size | payload | meaning / converter |
|------|------|---------|---------------------|
| 0x01 | 4 | `u16` | road surface cover → `enConvertSurface` @0x0091cd84 (table below) |
| 0x03 | 3 | `s8` | relative elevation, signed byte pass-through (`u16ConvertElevation` @0x0091fd40) |
| 0x04 | 8 | `{u16, u8, u8, u8, u8}` | DCM (Digital City Model / 3D city-model) info. `bReadWithOutBase`@0x008d9784 reads `u16@+2, u8@+4, u8@+5, u8@+6, u8@+7`. `u16ConvertDCMInfo`@0x00920688 then emits to FastMap: `(u8@+4)×10`, `(u8@+5)×10`, `class = u8ConvertDCMClass(low byte of u16@+2)` @0x0091cf8c (`0x00`→1, `0x20..0x32`→2..20, else 0), then a `0` byte. `u8@+6`/`u8@+7` are read but unused by the converter. So the **u16 field selects the DCM class**, and payload bytes 2–3 are two ×10-scaled params (rendering-side: model type/height/extent) |
| 0x10 | 4 | `u16` | water: low nibble = class code, high nibble = type code (`u8ConvertWaterClass`/`Type` @0x0091cefc/d110; tables below) |
| 0x11 | 8 | `{u16, u32}` | road info — bit layout below (`u16ReadRoadInfo` @0x009200e8 does `bCheck(6)` + readU16 + readU32) |
| 0x14 | 8 | `{u16 textRef, u16 mid, u16 status}` | road number: `textRef*4` = **text record** offset (bare digits); `status` bits 4–5 mode / bit 6 → `u8RoadStatus2Status` @0x0091d3d8; `mid` + `status&0xF` feed the name-prefix interning (`u32SkipPrefixOffset`) — shared name prefixes are stored once |
| 0x21 | 4 | `u16` | city type — bit layout below (marks city/village name POIs) |
| 0x22 | 4 | `u16` | rest area → u8 pass-through |
| 0x23 | ? | ? | parking (category flag only in the converter) |
| 0x30 | ? | ? | fuel / gas station (category flag only) |
| 0x31–0x33, 0x42–0x46, 0x49 | 4 each | `u16` per element | "list" = a **run of consecutive same-type annotations** in the stream (`u16ConvertListOfAnnotation` @0x0091f8b0 pops following same-type entries and writes a count u8 first); each element converts via `u8ConvertMap2FastMap(u16, type)` |
| 0x34 | ? | ? | restaurant (category flag only) |
| 0x35 | ? | ? | brand chain (category flag only) |
| 0x41, 0x47, 0x48 | 4 | `u16` | specification → `u8ConvertMap2FastMap(u16, type)` @0x0091f830 |
| 0x51 | ? | ? | POI image id |
| 0x52 | ? | ? | POI landmarks |
| 0x7A | 4 | `u16 v` | name — **text record** at `v*4` (block-relative), multi-language |

#### Road info (0x11) bit layout

Payload `{u16 w, u32 d}` (on-disk order: u16 first). Verified from the call
conventions of both consumers (`u8RoadInfo2Flags(u32,u16)` @0x0091d398 and
`u16ConvertRoadClass(u16,u32)` @0x0092015c, checked at assembly level):

| field | source | use |
|-------|--------|-----|
| network class | `w & 7` (bits 0–2) | `u8GetUserDefRoadClass` |
| intersection-free | `d >> 10 & 1` (**u32** bit 10) | `u8GetUserDefRoadClass` (traced as "intersection free") |
| flag byte bit 0 | `w >> 10 & 1` (**u16** bit 10) | output flags |
| flag byte bit 1 | `(w & 0x800) == 0` (inverted u16 bit 11) | output flags |
| flag byte bit 2 | `(w & 8) == 0` (inverted u16 bit 3) | output flags |
| flag byte bits 3/4/5/6 | `d & 1 / 2 / 8 / 4` (u32 bits 0/1/3/2) | output flags |

Note the two distinct "bit 10"s: u16 bit 10 feeds the flag byte, u32 bit 10 is the
intersection-free flag.

#### Road info (0x11) sub-attributes

The *same* `{u16 w, u32 d}` also carries road sub-attributes, unpacked by
`u16ConvertRoadSubAttribs(d,w)` @0x0091eb60 (each gated on its own field being non-zero):

| sub-attribute | source field | values (`u8Road*2RoadSubAttr*`) | OSM mapping |
|---------------|--------------|----------------------------------|-------------|
| toll | `w & 0x30` (bits 4–5) | `0x10`→3, `0x20`→2, `0x30`→1, else 0 (`u8RoadToll2…` @0x0091d308) | non-zero → `toll=yes` |
| ferry | `w & 0xC0` (bits 6–7) | `0x40`→3, `0x80`→2, `0xC0`→1, else 0 (`u8RoadFerry2…` @0x0091d338) | non-zero → `highway=ferry` |
| closed (DtClose) | `w & 0x300` (bits 8–9) | `u8RoadDf2RoadSubAttrDtClose` @0x0091d368 | kept raw (`tm:closed`), not OSM-mapped |
| **road type** | `w & 0xF000` (bits 12–15) | see table below (`u8RoadType2…` @0x0091d264; `bIs*` one-cell predicates) | link / roundabout |
| display class | `d & 0xF0` (bits 4–7) | `u8RoadDispClass2…` @0x0091d178 | kept raw, not OSM-mapped |

Road type values (confirmed via `rnw_tclOnecellInternal::bIs*` on `u8GetRoadType`):

| value | meaning | predicate | OSM |
|-------|---------|-----------|-----|
| 1 | long ramp | `bIsLongRamp` @0x008889c4 | `<class>_link` (unclassified → `service_link`) |
| 2 | roundabout | `bIsRoundAbout` @0x008889dc | `junction=roundabout` |
| 3 | parallel road | `bIsParallel` @0x00913c64 | (kept raw) |
| 9 | interconnect / slip road | `bIsInterconnect` @0x008889f4 | `<class>_link` |

So a ramp/interconnect on a motorway becomes `highway=motorway_link`, on a trunk
`trunk_link`, etc. — matching the OSM convention for slip roads / interchange ramps.

#### City type (0x21) bit layout

Payload `u16 v`, split by `u8ConvertCityType2{DisplayLvl,Inhabitants,AdminLvl,NameOverlapping}` @0x0091d4c4/d3fc/d588/d670:

| bits | field | values |
|------|-------|--------|
| 0–3 | display level | 1..14 (0 = none) |
| 4–7 | size class (inverted scale) | `0x1` = largest … `0xC` = smallest (→ internal 12..1); other values → 0 |
| 8–10 | admin level | 0..14 → internal 1..15; other → 0 |
| 15 | name-overlapping flag | set → 1, clear → 2 |

#### Water (0x10) value tables

class = `u16 & 0xF`: `0..5` → internal 2..7, `0xF` → 1, else 0.
type = `(u16 >> 4) & 0xF`: `0,1,2,3,4,6` → internal 1..6 (in that order), else 0.

#### Surface (0x01) value table

`enConvertSurface(u16)` maps raw → internal enum:
`0x11`→1, `0x10`→0xF, `0x20`→0xB, `0x21`→0xC, `0x22`→0xD, `0x30`→0x17, `0x31`→0x15,
`0x32`→0x16, `0x33`→0x18, `0x40`→0x20, `0x41`→0x1F, `0x42`→0x21, `0x43`→0x22,
`0x50`→0x2A, `0x51`→0x29, `0x52`→0x2B, `0x53`→0x2C, `0x54`→0x2D, `0x60`→0xE,
 `0x61`→0x33, `0x62`→0x34, `0x63`→0x35, `0x64`→0x36; else 0.
 Observed raw values in N6E1 L2: 82–100 (0x52–0x64) dominate.

> **Writer status.**
> - **`0x14` road-number: EMITTED** by `osm2map`. A road carrying OSM `ref` gets a `0x14`
>   annotation right after its `0x11` (`annotDesc.count = 2`), with `mid=0`/`status=0` (the neutral
>   codes the decoder reads back as `tm:roadnum_mid/status=0`) and `textRef` pointing at a
>   name-format text record — the same `u16AddText` record shape Bosch's own converter uses for road
>   numbers (see `u16ConvertRoadNumber@0x0091fa48`) and that POI names already render on the head
>   unit with. Validated end-to-end: `krzeszowice` `ref=79` (DK79, netclass 2) → 61 road cells →
>   decoded back to `ref=79`; selective (61/6541 line cells, not global).
> - **`0x21` city (settlement label sizing): EMITTED** by `osm2map` for `place=*` nodes, which are
>   written as **feature low `0x01`** (the settlement marker) + a `0x21` annotation (`count` up by 1).
>   Bits (decoded from stock N6E2 city POIs): **display** bits 0-3 = label min-zoom (smaller = shown
>   earlier), **size** bits 4-7 = importance (1 biggest … 15 hamlet), **admin** bits 8-10 = `1` for a
>   voivodeship capital (KRAKÓW/KATOWICE) else `7` (all ordinary municipalities), **bit 15** = name
>   overlap. Observed stock size↔importance: cities ~5-6 (display 4-6), towns 9-11, villages 12-15
>   (display capped 12). Writer maps `city`→disp5/size6, `town`→9/9, `village`→12/13, `hamlet`→12/15,
>   `suburb`→11/11, `admin=7`. Validated end-to-end: `krzeszowice` `place=town` (Krzeszowice) →
>   decoded `disp=9 size=9 admin=7 feat=1`; 25 place nodes → 25 city POIs. tmcheck PASS.
> - **`0x01` surface: NOT emitted.** `enConvertSurface` only gives raw-code → internal-render-enum;
>   there is **no confirmed raw-code → OSM-surface-material** mapping, so choosing a code for
>   `surface=asphalt`/`paved`/… would be fabrication. The per-code meaning is also unobserved
>   (values 0x52–0x64 dominate with no label). Additionally the annotation distribution shows `0x01`
>   on **polygons** (428), not road lines — so road-surface is not even a stock line annotation here.

Key facts (all verified on data):

- Text positions are **block-relative** (`v * 4` from the block start), not file
  offsets — `u32CalcPos(v) = v << 2` @ `0x008d5654`.
- The MAP header field `u16@2 = 52` is only an info-string table (copyright etc.),
  **not** a global text-section table.
- The converter emits the decoded payloads as OSM tags: `tm:surface` (0x01 raw u16),
  `tm:elev` (0x03 s8), `tm:water_class`/`tm:water_type` (0x10 nibbles),
  `tm:netclass`/`tm:xfree`/`tm:roadinfo` (0x11; `tm:roadinfo` = raw `u16:u32` hex for
  round-trip fidelity), `tm:city_display/size/admin/overlap` (0x21). Raw values are
  preserved so an OSM→TravelMap writer can reconstruct the exact bytes.

### Text record grammar

Two shapes (100 % parse rate over 64,027 name annotations on Poland L2+L3):

```
name:   {u8 n_langs, (u8 variant, u8 len) x n_langs, utf8 str1 .. strn, 0x00}
number: {ascii digits, 0x00}
```

Multi-language variants are stored in one record (e.g. `["SZÁPÁR", "SZÁPÂR"]`).
**`variant == 0xA7`** in stock (a "real display label" flag; `0x00` = "no display string"). This byte is
**write-critical and safety-critical**: `map2osm` ignores it (a wrong value still decodes), so round-trips
never caught this. CAR-VALIDATED (trial #12 vs #10/A/B/C): `0x00` boots and draws nothing (label present
in file, skipped by the render engine); `0xA7` **REBOOTS the head unit** when the string payload is raw
OSM text (mixed case / UTF-8 diacritics), because it hands the payload to Bosch's text engine, which
then walks off the buffer. `0xA7` is only safe on **Bosch-normalised labels: UPPERCASE ASCII with
diacritics stripped (KRAKOWIE, KOSCIUSZKI), often a 2-variant record**. Our writer therefore defaults
the flag to `0x00` (no-label, safe) until label normalisation is implemented — see `name_str_flag()` /
`OSM2MAP_TEXTVARIANT`.
Interning on the write side: `u16AddText` / `u16DumpToMem` @ `0x008e0584`.

### Annotation type distribution (N6E2 L2+L3 sample)

| kind | top types |
|------|-----------|
| POI | name 21,749 · city 8,078 · list(0x45/0x41/0x46/0x44/0x47/0x48) · parking 481 · restaurant 132 · fuel 334 · brand 136 · rest_area 112 |
| line | water 17,349 · name 10,125 · roadinfo 5,316 · roadnum 2,105 |
| poly | dcm 61,567 · name 7,077 · water 4,235 · surface 428 |

---

## 9. Converter — Usage

```
map2osm_rs   <IDX_file | directory> [-r CODES] [-l LEVELS] [-b W,S,E,N|none] [-o OUT_DIR]
```

- `-r N6E1,N6E2` — exact region-code filter (when a directory is given); omit for all 411 regions.
- `-l 0123` — levels to convert (default `123`; **L0 works** — whole-region outline level).
- `-b W,S,E,N` — bounding box in decimal degrees (west,south,east,north); only tiles whose
  extent overlaps the box are converted. Same syntax as `rnw2osm_rs`. `none` (default) = no filter.
- `-o DIR` — writes `DIR/<REGION>_L<level>.osm` (OSM XML; omit for stdout).

Output (see https://wiki.openstreetmap.org/wiki/OSM_XML): POIs → `<node>` with tags,
lines → open `<way>`, polygons → closed `<way>` (first node repeated); unique coordinates
are deduplicated into single `<node>` elements written before the ways; every object
carries `id` + `version="1"` + `timestamp` (dataset date) so JOSM/osmium accept the file.
Tags, three layers:

1. **Standard OSM** (semantic overlay, best-effort, so JOSM/routing can use the file):
   `name`, `name:alt` (`'; '`-separated variants), `ref`, and — driven by the POI feature
   code (§7) — a category tag per point: `amenity` (parking/fuel/restaurant/car_rental/
   school/bar/pharmacy/bank/place_of_worship), `shop` (car/supermarket), `tourism=hotel`,
   `leisure` (sports_centre/park), `office=company`, `railway=station`. Plus `place`
   (city/town/village/hamlet from city size class), areas — driven by the polygon feature
   code (§7): `landuse` (residential/commercial/cemetery/grass) and `natural=wood`+
   `landuse=forest` (forests), `natural=water`+`water=lake` (closed water polygons),
   `waterway` (river/canal/stream/ditch from water type), and roads:
   `highway` (motorway…service from network class, or `rest_area`; `<class>_link` for
   ramps/interconnects; `ferry` for ferry routes), `toll=yes`, `junction=roundabout`.
   Only feature codes confirmed by evidence are mapped; the exact code always stays in
   `tm:feature`. Two things are deliberately NOT mapped to standard OSM keys; Ghidra
   confirms why (DAPIAPP.OUT):
    - **oneway / direction** is *not stored in the MAP display format at all* — it lives only
      in RNW routing data (`rnw_tclLocalOneCellRef` u16, bits 13-14: both-clear = two-way,
      bit13 = same-direction-only, bit14 = reverse-direction-only; `bHasLaneIn*Directions`).
      The MAP line's road attribute (0x11) carries no direction, so it cannot be mapped.
    - **surface** (0x01) is fully decodable (`enConvertSurface`, table in §8) but the binary has
      no material-name table for the resulting enum, so an OSM `surface=` value would be a guess;
      the raw code stays in `tm:surface`.
   Road sub-attributes (link/ramp, toll, ferry, roundabout) ARE mapped — see the 0x11
   sub-attribute table in §8 for the confirmed bit layout and road-type values.
2. **Converter properties** (custom namespace): `tm:kind/tm:layer/tm:tile/tm:profile/
   tm:state/tm:feature/tm:type`.
3. **Decoded annotation payloads** (see §8) — raw values preserved for an OSM→TravelMap
   writer: `tm:surface`, `tm:elev`, `tm:dcm`(+`tm:dcm_class`), `tm:water_class`,
    `tm:water_type`, `tm:netclass`, `tm:xfree`, `tm:road_type`, `tm:toll`, `tm:ferry`,
    `tm:closed`, `tm:roadinfo`, `tm:roadnum_status`,
    `tm:roadnum_mid`, `tm:city_display/size/admin/overlap`, `tm:rest_area`,
   `tm:spec:<type>` (grouped list/specification runs), and `tm:raw:<type>` (lossless
   hex fallback for any other annotation type, e.g. parking/fuel/restaurant/brand POI
   categories). Note: polygon cells can carry many annotations (e.g. thousands of DCM),
   so a lossless conversion is large — use per-region output and/or gzip.

Examples:

```
# whole regions, all default levels
map2osm_rs .../DATA/DATA/MAP -r N6E1,N6E2 -l 123 -o /tmp/pl

# only tiles overlapping a bounding box (Krzeszowice), detail level
map2osm_rs .../DATA/DATA/MAP -r N6E2 -l 3 -b 19.50,50.05,19.88,50.28 -o /tmp/krz
```

Performance: N6E2 L2 ≈ 560 MB of OSM XML in ~6 s. A full world conversion is multi-GB —
use per-region files and/or gzip the output.

Road-name enrichment pipeline: `rnw_extract_rs [CCP_DIR] RNW.jsonl`, then
`rnw_join_rs RNW.jsonl <REGION>_L2.osm <REGION>_L2_rnw.osm` adds `name`/`name:alt` and
`rn_class/rn_netclass/rn_link/rn_sec` tags to the road ways (OSM XML in and out).
See `RNW_format.md` for the RNW format.

---

## 10. Verification

- **Whole N5E2, L1+L2+L3:** 29,470,390 points — **0 outside the tile BBox**.
- **Landmarks (N6E1+N6E2, L2):** all HIT — Warsaw, Kraków, Gdańsk, Wrocław, Szczecin,
  Poznań, Łódź, Białystok, Gdynia; Vistula river (Warsaw/Kraków/estuary), Oder river
  (Wrocław/Szczecin); Baltic coast (Kołobrzeg, Sopot, Ustka).
- **Rivers** appear as `line` (feat 0x31/0x32/0x33), **coast/sea** as large `polygon`,
  **cities** as `poi`.
- **L3 around Warsaw:** 26,806 cells / 75,344 points — building footprints (polygon feat
  0x209C–0x259C) plus dense POI, i.e. street-level detail.
- **Names (N6E1+N6E2, L2):** 15.3 s for both regions; N6E1 733,759 features / 402,105
  named, N6E2 408,265 / 200,855 named (23,163 with road `ref`); 114,947 named lines,
  360 named polygons. City spot-checks all HIT: WARSZAWA, KRAKÓW, GDAŃSK, BIAŁYSTOK
  (N6E2), WROCŁAW, SZCZECIN, POZNAŃ (N6E1); village/street names around Warsaw verified
  against real geography.
- **Layers:** e.g. N6E1 L2 = road 252,919 · area 225,383 · poi:city 143,725 · water 68,670
  · water_area 30,689 · poi 7,148 · poi:rest_area 4,829 · line 396. Water lines carry real
  stream names (GYÁLI-PATAK, GERJE), roads carry real Hungarian refs (8227, 8208…),
  `poi:city` are real village names (SZÁPÁR, JÁSD, TÉS).

### Known pitfalls (fixed during the reverse engineering)

1. Region name = IDX name **without** the `AA` suffix (for locating the MAP files).
2. Upper BBox border = `south + rel_n` (not `north + rel_n`).
3. `MAPWORLD.MAP` partition read starts at 0x28, 8 B per row (earlier off-by-one-byte bug).
4. Region filter — exact match, not substring (`N6E1` ≠ `N6E10`).
5. **POI cell deltas are SIGNED s16** (list-2 cells: `{state, feature, s16 dlon, s16 dlat, ...}`).
   Reading them unsigned shifts every POI with a negative delta by up to
   `65535 << shift` PAU (5.6° at L1, 0.7° at L2; e.g. LE MANS would land in the North Sea).

---

## 11. Generating your own `.MAP`/`.IDX` — write-side status

The firmware contains only the **reader** (`DAPIAPP.OUT`). The build tools that emit these
files ("TpMap2 (Map-Data) for TravelMap", "Linker-SW / MK10_2021.1" per the info strings)
are not present, so a byte-exact write layout must be inferred from the reader + the data.
What a generator must produce, and what is still not fully pinned:

### `.IDX` — fully known (writable)

- 32 B header: `binOff`, `spare`(=32), `west/south/east/north` (PAU), `partOff`(×4). Stock N6E2:
  `partOff`=**0x7e** (partition table at pt=`partOff*4`=0x1f8), `binOff`=**0x234**.
- Descriptive block `[0x20 .. partOff*4)`: u16 offset list + packed ASCII metadata — like the MAP
  info region, **byte-identical across every region/profile** (only date digits vary). ⇒ copy a stock
  prefix verbatim as a template and patch only `west/south/east/north` (+ optionally `binOff`).
- Partition table: 4 × 12 B at `partOff*4`.
- Tile tables: L0 @ `binOff`, L1/L2/L3 @ `u32b>>8`; slot = `{u16 regProf, u16 length(words),
  u32 offset}`.
- **`multi` slot (decoded this pass):** `{u16 0x4000, u16 count, u32 ptr}` → `count` × 8-byte
  real slots at `ptr`. Used when one tile spans several profile files (e.g. L0 = whole region →
  one sub-slot per profile). Verified: N6E1AA L0 → 9 sub-slots, each resolving to a valid block
  (`0xFFFF…` marker) in its `<REGION>1<prof>.MAP`.
- `regProf` field = `0x400 | prof` (bit 10 is a flag; the file name comes from the two base32
  digits of `prof`); bit 14 = multi, bit 15 = empty.

### `.MAP` header — resolved: copy it verbatim (write-side)

- 32 B header: `binOff`(first block), `infoTbl`(=0x34), `fileSize`, `west/south/east/north`,
  then 4 × u16 (`@0x18`=**8**, `@0x1a`=**4**, `@0x1c/0x1d`=**04 04**, `@0x1e`=`0x8400|regProf`).
  `binOff` is **0x7bc in ~94 % of shipped MAP files** (rest are the ±8 B neighbours) — a generator
  must NOT shrink it to `0x40`.
- Info-string / metadata region `[0x20 .. binOff)` (~1.9 KB): an info-table at `infoTbl`=0x34 plus
  packed ASCII (copyright, build date/time, product "TpMap2 (Map-Data) for TravelMap", project).
  **Verified byte-identical across every shipped region/profile** (diff of 40 canonical files: only
  six date digits at 0x7f/0x81/0x82/0x44f/0x451/0x452 vary). ⇒ it is static Bosch build metadata and
  must be **copied verbatim from a stock file**, never regenerated as zeros.
- `@0x1e` (profile) is not read by the runtime header ctor (`dap_map_tclMapFileHeader` @0x008d8e1c,
  stores only 0x00–0x1C); profile comes from the IDX slot — but we still patch it for consistency.

- Binary blocks `[binOff .. fileSize)`: contiguous, 4-byte aligned. Each =
  `{u32 marker = 0xFFFF | (len<<16)}` + 3 × `{u16 start, u16 count}` + cells (12 B) + point pool
   + annotation/text (§5/§8). `start[i+1] = start[i] + count[i]*3`; `len` (words) spans to the
   next block.

### `.TCI` — Tile Cluster Index, fully known (writable)

Per-MAP-file sub-index (`<REGION>1XX.TCI`, one per profile). Decoded from `DAPIAPP.OUT` classes
`dap_map_tclTCIHeader` / `TCIPartition` and loaders `u16LoadPartitionTable` /
`u16LoadClusterIndexTile` / `u16GetClusterId`, confirmed against stock N6E2 10I + 11A:

```
[0x00] header (20 B): u16 f0=0, u16 f1=92, u32 fileSize, u16 partOff=0x84, u16 partCnt=4,
                      u16 f5=122, u16 f6=16, u16 f7=12, u16 f8=106      (format constants)
[0x14] descriptive block (112 B): copyright / build stamp / "TILE_CLUSTER_INDEX" / "TPNAV2"
[0x84] partition table: 4 × {u32 level, u32 tileCount, u32 sectionOffset}
[0xb4] per-level tile records: tileCount × {u16 primCl, u16 cl, u32 clusterOffset}
[EOF-] cluster pool (geometry — only for profiles that use clusters)
```

- `partOff` (header @0x08) = byte offset of the partition table; `partCnt` (@0x0a) = 4, one entry
  per level — `u16LoadPartitionTable` errors (`0x218`) if it is not 4. N6E2 tileCounts =
  1 / 25 / 2500 / 250000 (same grid as the IDX).
- `sectionOffset[L]` = byte offset of level L's record array = `180 + Σ_{j<L} tileCount[j]·8`
  (L0@180, L1@188, L2@388, L3@20388 for N6E2).
- A tile record is all-zero (`primCl=cl=clusterOffset=0`) when the tile has no cluster data;
  otherwise `clusterOffset` points into the trailing pool.

**When it is / isn't needed.** `dap_map_tclIdController::u16GenerateTileIds` resolves a region's
tiles two ways: positive-index tiles go through the `.IDX` → `.MAP` block path; negative-index
("cluster") tiles go through the TCI via `u16GetClusterId`. If the TCI file is absent, that call
returns error `0x307`, logs `"Could not read tci file"` and skips just those cluster tiles — it
does **not** abort the whole region. Discovery is a wildcard `*.tci` directory scan
(`u16InitTciFileList`), so no specific TCI file is mandated to exist.

**Key fact:** for profile **10I** the stock TCI's cluster pool is 100 % empty (no geometry) — all
of 10I's data flows through the `.IDX` path. Profile 11A, by contrast, carries a real ~1.7 MB pool
(same structure, non-empty records). So which profiles actually use clusters varies per profile;
the file format is identical either way.

**Consequence for swapping:** to replace routes/objects you only swap `<REGION>AA.IDX` +
`<REGION>1XX.MAP`. The TCI is **not required** — leave the stock `.TCI` in place (empty for 10I,
contributes nothing) or omit it (worst case: harmless `0x307` logs). Do **not** copy a TCI from
another profile/region: its `clusterOffset`s point at that file's own layout and would be wrong.

`osm2map_rs::emit_tci` generates a structurally-valid, all-empty TCI (header + descriptive block +
partition table + zeroed record arrays) byte-identical to the stock 10I prefix; it round-trips
byte-exact through CPRNAV. Emit it only if you target a cluster-using profile or want the file
present to avoid the `0x307` logs.

### Still not fully pinned (all avoidable for a minimal working file)

- **REBOOT ROOT CAUSE (confirmed empirically = EMPTY-tile slot encoding).** Trial #04 rebooted the
  head unit even with byte-correct headers, a pad4 container and a self-consistent IDX — so the fault
  was in the tile-table slot *encoding*, found by diffing stock `N6E2AA.IDX` against osm2map's. The
  rule: an **empty** tile MUST be written as `regProf = 0x8000` exactly, with `len=0`, `off=0` (raw
  bytes `00 80 00 00 00 00 00 00`). Bosch does this for all 67k+ empty L3 tiles — it deliberately
  leaves the profile bits **cleared**, so even if a slot is ever followed, bit15 selects a
  *non-existent* file (`base32(0)`→`N6E210…`) that simply fails to open. osm2map had been OR-ing the
  profile into the marker (`0x8000|0x412 = 0x8412`), which points an empty tile at the **real**
  `N6E210I.MAP` with `off=0`; the reader then treats the MAP *header* as a block and walks its bytes as
  cells → OOB read inside `u16ConvertCells` @0x008d7660 → reboot. **Fix:** emit empty = bare `0x8000`,
  and a `multi` header = bare `0x4000` (profile bits cleared, as stock). Same class of "don't leave
  stray profile bits in a marker slot".
- **Why the info region / fabricated header are NOT the cause.** The geometry path —
  `vConvertMapData` @0x00847604 → `u16Convert` @0x008d8248 → `u16ConvertCells` — is driven **only** by
  the IDX-provided `MemBlockDesc` (block ptr/len) plus tile BBox/shift from the partition table; it does
  not read `binOff`, `infoTbl` or `[0x20 .. binOff)` (header ctor @0x008d8e1c stores no pointer there).
  So copying the stock prefix verbatim is good hygiene but never rendered/never faulted — and a region
  whose **IDX is left untouched** (trial #03) was always safe, which is exactly why #03 worked while
  every osm2map IDX swap (#02, #04) rebooted. `diag/tmcheck.py` (§10) now enforces the empty/multi
  bit-patterns + block bounds for every slot; it FAILs on the old #04 IDX and PASSes on all of stock.
- **Header @0x18 / @0x1a / @0x1c — RESOLVED for writing:** constants (8, 4, and binOff-derived); the
  safe strategy is to inherit them from the copied template. Consumption by the runtime still not
  individually traced, but copying them verbatim matches every shipped file.
- **Profile model — CORRECTED (empirical, 411 regions + Ghidra).** A profile is **not** a category /
  display layer. Two findings from the full map set:
  1. **`0I` is universal and special.** It exists in *every* region (387/411 ship *only* `0I`) and
     holds **hydrography only** — coastline/water lines + water-area polygons, **zero POI cells and
     zero settlement/landuse polygons**. Ocean regions need nothing else.
   2. **Non-`0I` profiles are geographic shards, not categories.** In the ~6 dense countries (N4E2/3,
      N5E1/2, N6E1/2) a region splits into several shard profiles; at L2 most tiles resolve to exactly
      one shard and every shard carries the **same *kind* of content** (roads + POI + landuse), but the
      per-shard **mix skews with the area's character**, not just volume. Empirical N6E2 full-region
      fingerprint (`diag` scan over all slots, cell/feature split geo vs point): `1A`=160 MB (dominated by
      area/building polys `..9c`, the dense core), `02`=35 MB (POI-heavy urban), `0H`=23 MB
      (geometry/area + boundary-annot heavy, few POI → rural), `0E`=18 MB (balanced). A pure-water tile →
      `0I` alone; a land tile with water → `<shard>` + `0I`; the L0 root aggregates all profiles.
   So Bosch partitions a region's tiles across several named data buckets (volume/geography-driven), with
   `0I` held out as the hydro overlay. **osm2map `#07` still lumps ALL land into a single shard (`02`) +
   `0I`** — better than the old one-file-everything, but not yet a multi-shard layout (N6E2 ships 4 land
   shards). There is **no** global "category → profile id" map and **no** profile literally named `109` —
   earlier examples to that effect were wrong.

- **Profile ids are internal to the region — there is NO on-disk map-profile registry (task C).** The only
  region/profile metadata files are `DATA/DATASET.CFG` (`DATASET_ID{1758962541}`, `USED_COMPRESSION{5}`,
  `DATABASE_CONFIG{'MAP'|'/MAP/''|'10.23'}`, and a small country-level `REGION_CONFIG`) + `MEDIUM.CFG`, and
  `DATA/DATA/MISC/tp_meta.dat`. The `resinf` “RegionMetaInfo/ProfileMetaInfo” path
  (`vProcessEvalRegionMetaInfos` @0x008b43c0 reads `…/data/data/misc/tp_meta.dat` via
  `u16CreateFullAvailMetaFileAccessPath` @0x008ae808, parsed by `dap_tclMetaDataCtrl::u16EvalAnnotations`)
  lists **Traffic-Provider profiles** named `INV,CCP,MRP,IRP,ALPS,TGP,ALP` (matching INFO.TXT “TMC (CCP/IRP…)”)
  — NOT the MAP display suffixes (`0I/02/…`). So it neither whitelists per-profile annotations nor constrains
  our map profile ids, and a `.MAP`/`.IDX` swap does **not** touch `DATASET.CFG`/`tp_meta.dat`.
  ⇒ the fatal-`0x204` dataset/metadata-eval path is ruled out for our edit. Map display profiles live only in
  each region's `AA.IDX` regProf words (+ the per-file `@0x1e` word), with the descriptive info-block copied
  verbatim from stock — nothing external to keep in sync besides emitting only ids that exist as files.
- **Renderer is profile-agnostic (procmapengine.out deep-dive).** `opt/bosch/processes/procmapengine.out`
  contains **no `Profile` symbol at all**; drawing works on FastMap `map_tclMapObject`s selected purely by
  **feature type + display/detail scale** (`bGetFeatureStatus(ren_tenMapFeature,…)`, `LayoutScene(...,
  map_tenScaleID)`, `CreateMapObject(map_trMapObjectData)`, `DetermineMapObjects`). The MAP *profile* only
  decides which file the geometry was read from; after `u16Convert` it is discarded. Consequence: authoring
  land in `0I` vs a shard is **invisible to the renderer** — it cannot change styling or fault rendering. So
  the two-profile split (#07) improves *file-layout fidelity* but, on its own, is unlikely to be the reboot
  fix; the reboot must originate upstream of the FastMap output (see remaining leads below).

- **Faithful file layout (`#07`).** Bosch keeps `0I` hydrography-only and puts land into separate shard
  profiles; no shipped region ships a lone `0I` carrying roads/POI. osm2map previously did exactly that, so
  `#07` now emits `10I` (hydro only) + `02` (land), each L2 tile → its shard (+ `0I` when it has water). This
  matches Bosch's layout and cluster/overlay conventions; since the renderer is profile-agnostic it is a
  *fidelity* change, **not** a proven reboot fix. **SUPERSEDED by `#09`:** the actual `#07`/`#08` reboot was the
  water-line feature-code bug (below), and stock N6E2 keeps *inland* water in the land shard, so the validated
  default is now single-profile `02` (`OSM2MAP_HYDRO=land`); the `10I` hydro-overlay split stays available only
  for coastal regions (`OSM2MAP_HYDRO=overlay`, untested on car).

- **Geometry is NOT the fault vector (offline reader-model verification).** Decompiling the actual
  path `vConvertMapData` @0x00847604 → `u16Convert` @0x008d8248 → `u16ConvertCells` @0x008d7660 shows
  every read goes through a **bounded data accessor** sized from the IDX `MemBlockDesc{ptr,len}`; a
  short/over-long read returns an error code (`0x321`/`0x218`) and the tile is skipped — it does **not**
  SIGSEGV. The block header it expects (after skipping the 4-byte marker) is exactly our `3×{u16 start,
  u16 count}` for lists 0/1/2, then point pool + texts — confirming our writer's layout field-for-field.
  Consequence: any output that (a) encodes slots to stock bit-patterns, (b) keeps every block inside its
  MAP file (`off+len*4 ≤ filesize`), and (c) uses valid feature/annotation codes cannot fault here. `#05`
  satisfied all three (tmcheck PASS) yet still rebooted ⇒ the cause is **outside** geometry.
- **Feature/annotation codes are all safe + legal.** The dispatchers `u8ConvertFeature2{Poly,Line,POI}Type`
  @0x008d56a0/0x008d582c/0x008d5a10 return a FastMap **type** via switch/if-ranges (default → `0`), never
  index a table by the raw feature ⇒ no OOB from feature codes. Our `#07` scan: poly low ∈
  {0x38,0x39,0x3A,0x2B,0x48,0x9C}, line low = {0x30}, POI low ⊂ valid `{1-9,0x10-0x17,0x21,0x22}`, feature
    high byte = 0 everywhere; annotations only {0x10,0x11,0x7A}. So `feature`/annot cannot crash the renderer
    and none of our POIs fall to an unmapped type. **CORRECTION (later #09):** the feature *dispatcher* never
    faults, but the code still selects the *reader*, and a code/annotation **mismatch** (water line written as
    road `0x30` while carrying `0x10`) mis-sizes the record → reader OOB → reboot — "individually valid codes" ≠
    "safe in combination"; see the water-line bullet below. NOTE (corrected §7): the polygon high byte only feeds the
   **clamped** `PolygonConfigMatrix::u16GetDisplayScale@0x8d9b08` (no OOB), so even a high byte of 0 is not a
   fault source in DAPIAPP — reinforcing that the reboot is downstream. Combined with tasks A–C (bounded
    reader; no map-profile registry / fatal-`0x204`; profile-agnostic renderer), **every DAPIAPP data-path
    component we can model offline is clean (all reads bCheck-guarded, all table indices clamped → error codes,
    never faults)**; the tile-id / partition loader upstream of `u16Convert` is likewise fully bounded (see
    remaining leads), leaving only the unlikely CPRNAV_2 decompress→hand-off boundary and the
    **procmapengine renderer** (consumes the converted FastMap polygons) as unverified suspects.
- **RESOLVED (car test #06c3): the reboot was a MISSING `0x04` DCM annotation on residential (`0x9c`) polygons, faulting the renderer.** The
  one suspect above is confirmed: `map_tclMapElm_Landuse_Area::ReadFrom@0x003ebc64` dereferences a
  style/config entry (`configTable[off]` → `*(float*)(pmVar13+0x38)`) that only the polygon's annotation
  supplies; with `annotDesc=(0,0)` the entry is absent → hard fault → reboot (panic_on_oops). This is
  a polygon-only path (line/POI cells carry their own annotations and render). It is NOT a feature-code,
  display-scale, sub-type, geometry, or quantity issue — all of those were excluded and #05/#06c passed tmcheck
  yet rebooted. **It is category-specific** (see §8 cross-tab): `0x04` (DCM) belongs only to `residential`
  (`0x9c`), which stock annotates ~100 %; grass/forest/cemetery are legitimately unannotated. **Fix: emit the
   `0x04` DCM (`08 04 20 00 11 00 00 00`) on `0x9c` polygons** — `osm2map build_block` default `OSM2MAP_POLY_ANN=cat`.
- **RESOLVED (car test #09n/#09p): a second reboot — the same misparse class — was WATER LINES carrying feature
  low `0x30`.** After #06c3 was fixed, #07/#08/#09f still rebooted. Isolation ladder `trials/09` (09a–09f, 09j–09p,
  one feature per rung, tmcheck-PASS) localised it: 09a–09e (land areas, POI, `0x21` cities, `0x14` road-nums) all
  render; **09f (water) reboots**. Splitting 09f: **09m = water *areas* → OK**, **09l = water *lines* → reboot**.
  Root cause: osm2map wrote water lines with feature low `0x30`, which `u8ConvertFeature2LineType` classifies as
  a **ROAD** line (type 4) → the road reader parses the record expecting a `0x11` roadinfo but finds our `0x10`
  water annotation → misparse → fault (identical failure mode to the `0x9c` case). Stock N6E2 puts inland water in
  the same shard as land and uses feature low **`0x20`** (type 3 = hydrography) for water lines. **Fixes: water-line
  feature `0x30`→`0x20` (`OSM2MAP_WATERLINE_FEAT`, default) AND default water into the land shard (`02`) rather than
  a separate `0I` — `OSM2MAP_HYDRO=land` (default), `overlay` retained for coastal `0I`.** Confirmed on car:
  09n (lines@0x20) and 09p (full map, single `02`) both boot + render. NOTE: every prior water reboot is now fully
  explained by the `0x30`-line bug, so the `0I`/multi-slot mechanism itself was **never proven broken** — `overlay`
  stays available for coastal regions pending a dedicated test. `#07`'s two-profile split above was a *fidelity*
  guess, not a fix; single-`02` inland is both stock-matching and car-validated.
- **`diag/tmcheck.py` is now a calibrated strict model.** Added: marker hi==`0xFFFF`, in-block marker len ==

  slot length (accessor window), multi sub-entry count ≤ 15. Calibrated to **PASS 17/17 diverse stock
  regions** (1–9 profiles, oceanic→dense; e.g. N4E2/N5E1/N5E2/N6E1) with zero false positives, still FAILs
  the old #04 empty-slot bug (`0x8412`), and PASSes `#07`.

- **Premium POI payload** (list 2, `feature & 0xF000 == 0xF000`): the 8-byte payload is skipped by
  `bSkipPremiumPOI` @0x008d5f00; its content is unknown. Avoid by not emitting premium POIs.
- **Annotation payloads 0x23 / 0x30 / 0x34 / 0x35 / 0x51 / 0x52:** size/payload undecoded (the
  converter treats them as category flags). Avoid by not emitting those types, or copy raw bytes
  from a reference file.
- **Tile-id enumeration / partition loader — now EXONERATED (fully traced + bounded).** Chain
  `dap_map_tclIdController::u16GenerateTileIds @0x8cd8e0` → `dap_map_tclTileLoader::u16ValidateTileId
  @0x8e3f00` → `u16LoadTileOffsetFromIdxFile @0x8e3cd8` → `u16ReadMultiIdxListDesc @0x8e3968`. Every loop is
  a size-bounded STL/container walk; the only raw file read is clamped like the geometry accessor. Concrete
  invariants the reader enforces (and that `tmcheck` now mirrors):
  - Slot word flags `(word & 0xC000)`: `0x0000` = single entry (`regProf = field>>2`, length = high half-word,
    offset from the companion u32); `0x4000` = multi → sub-list via `u16ReadMultiIdxListDesc`; **any other flag
    (incl. the `0x8000` empty marker) is silently skipped** ⇒ an empty slot never faults.
  - Multi sub-entry count `= *(u16)(desc+2)` guarded by `if (count > 0xF) → err 0xd`; it reads exactly
    `count × 8` bytes at `offset = *(u32)(desc+4)` through `u16LoadDataBlockConnectGlobal` inside a fixed
    0x4000 preload window (bounded accessor). No nested multi ⇒ no recursion blow-up.
  - Single/multi entries with `offset==0 || length==0` are dropped (`RegionList::u16Add`, not pushed as data) —
    this is the code path that made old `#04`'s bogus non-empty-looking empty slot (`0x8412`) invalid, and why
    `tmcheck` rejects it.
  Conclusion: with a `tmcheck`-clean output these loaders cannot OOB, so #04/#05 could not have rebooted here
  *if* they had satisfied these invariants (the empty-slot rule #04 did not).
- **Remaining reboot lead — container/decompression stage (CPRNAV_2), only and unlikely.** `#05` used a
  stock-valid container that decompresses byte-exact, but the decompress→hand-off boundary has not been
  independently confirmed. Everything else modelable offline (partition/tile-id loader, per-block reader,
  feature/annot codes, dataset/profile metadata, renderer) is now bounded/clean for a `tmcheck`-passing output.
  **Offline RE is exhausted — decisive progress needs the head unit: flash #07, then run the #06 isolation
  ladder.**

- **Checksum/CRC:** none observed in any header or block; unverified whether an external tool validates one.



### Minimal viable `.MAP`/`.IDX` pair (readable for geometry)

Get the IDX right (header + partition table + tile tables pointing at real block offsets), and
write the MAP binary blocks with correct markers / cells / points / annotations plus a
self-consistent 32 B header (`binOff`, `fileSize`, bbox). The metadata info-string region and the
premium / unknown-annotation payloads can be omitted or copied verbatim from a reference file.

The `.TCI` is **not** part of the minimal pair — for profile 10I it is empty and the geometry is
fully carried by `.IDX` + `.MAP`, so a swap only needs those two files (see the `.TCI` subsection
above).
