# Navigation Map Data Format — Bosch TravelMap / Nissan LCN2KAI (EUR 2021.Q1)

Complete documentation of the `.IDX` / `.MAP` / `MAPWORLD.MAP` file format, reverse
engineered from the `DAPIAPP.OUT` binary (Ghidra, project "Nissan Ghidra Project") and
verified against the firmware data.

Converter implementing this format: [`map2osm_rs/`](./map2osm_rs/) (OSM XML, Rust;
build with `cargo build --release`). Road-name enrichment: `rnw_extract_rs` +
`rnw_join_rs` (also OSM XML in/out).

---

## 1. File Overview

Source directory: `CRYPTNAV/DATA/DATA/MAP/` (411 regions, ~907 MB).

| File | Role |
|------|------|
| `<REGION>AA.IDX` | One per region. Tile tables (4 levels) pointing to blocks inside the MAP files. |
| `<REGION>1XX.MAP` | Eight per region (one per `regProf`). Contain the actual geometric data. |
| `<REGION>1XX.TCI` | Tile Cluster Index per MAP file (not needed for geometry decoding). |
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

The high byte of `feature` = **display scale** (visibility threshold).

### Lines (list 1) — code → type

| code (hex) | type |
|------------|------|
| 0x07 | 100 |
| 0x10–0x17 | 1 |
| 0x20 | 3 |
| 0x21 | 2 |
| 0x30–0x37 | 4 |
| 0x71–0x73 | 100 |

### POI (list 2) — code → type

| code (hex) | type |
|------------|------|
| 0x01–0x09 | 1–9 |
| 0x10–0x17 | 0xA–0x11 |
| 0x20–0x25 | 0x12–0x17 |
| 0x26 | 0x1B |
| 0x27 | 0x1C |

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

### Annotation types

Dispatch: `dap_map_tclAnnotationConverter::u16WriteAttrib` @ `0x00920744`.

On-disk payload layouts (verified against N6E1 L2 data + Ghidra; `size` = header byte,
payload = `size - 2` bytes):

| type | size | payload | meaning / converter |
|------|------|---------|---------------------|
| 0x01 | 4 | `u16` | road surface cover → `enConvertSurface` @0x0091cd84 (table below) |
| 0x03 | 3 | `s8` | relative elevation, signed byte pass-through (`u16ConvertElevation` @0x0091fd40) |
| 0x04 | 8 | `{u16, u8, u8, u8, u8}` | DCM (3D/city model) info — `dap_map_tclDCMAnnotation::bReadWithOutBase` @0x008d9784; first two values are written ×10, 3rd byte = class → `u8ConvertDCMClass` @0x0091cf8c (`0x00`→1, `0x20..0x32`→2..20), last 2 bytes unused by the converter |
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
name:   {u8 n_langs, (u8 lang, u8 len) x n_langs, utf8 str1 .. strn, 0x00}
number: {ascii digits, 0x00}
```

Multi-language variants are stored in one record (e.g. `["SZÁPÁR", "SZÁPÂR"]`).
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
map2osm_rs   <IDX_file | directory> [-r CODES] [-l LEVELS] [-o OUT_DIR]
```

- `-r N6E1,N6E2` — exact region-code filter (when a directory is given); omit for all 411 regions.
- `-l 0123` — levels to convert (default `123`; **L0 works** — whole-region outline level).
- `-o DIR` — writes `DIR/<REGION>_L<level>.osm` (OSM XML; omit for stdout).

Output (see https://wiki.openstreetmap.org/wiki/OSM_XML): POIs → `<node>` with tags,
lines → open `<way>`, polygons → closed `<way>` (first node repeated); unique coordinates
are deduplicated into single `<node>` elements written before the ways; every object
carries `id` + `version="1"` + `timestamp` (dataset date) so JOSM/osmium accept the file.
Tags: `name`, `name:alt` (`'; '`-separated variants), `ref`, plus the original
converter properties under a custom namespace: `tm:kind/tm:layer/tm:tile/tm:profile/
tm:state/tm:feature/tm:type`, and decoded annotation payloads (see §8):
`tm:surface`, `tm:elev`, `tm:water_class`, `tm:water_type`, `tm:netclass`, `tm:xfree`,
`tm:roadinfo`, `tm:city_display`, `tm:city_size`, `tm:city_admin`, `tm:city_overlap`.

Example (Poland):

```
map2osm_rs .../DATA/DATA/MAP -r N6E1,N6E2 -l 123 -o /tmp/pl
```

Performance: N6E2 L2 ≈ 560 MB of OSM XML in ~6 s. A full world conversion is multi-GB —
use per-region files and/or gzip the output.

Road-name enrichment pipeline: `rnw_extract_rs [CCP_DIR] RNW.jsonl`, then
`rnw_join_rs RNW.jsonl <REGION>_L2.osm <REGION>_L2_rnw.osm` adds `name`/`name:alt` and
`rn_class/rn_netclass/rn_link/rn_sec` tags to the road ways (OSM XML in and out).
See `new_file_format_rnw.md` for the RNW format.

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
