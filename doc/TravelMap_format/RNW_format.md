# RNW (Road Network) Database Format — Bosch TravelMap / Nissan LCN2KAI

Reverse engineered from `DAPIAPP.OUT` (Ghidra) and validated against the MAP
converter output (`new_file_format_eng.md`) for region **N6E2**
(18.00–36.00°E, 47.25–56.70°N: eastern Poland, Slovakia, NE Hungary, western
Ukraine/Belarus, Baltic coast).

Tooling: `rnw_extract_rs` (extractor) and `rnw_join_rs` (join onto MAP OSM XML) —
zero-dependency Rust; build with `cargo build --release`.

## 1. File organization

```
CRYPTNAV/DATA/DATA/RNW/CCP/<REGION>/NAV_ROOT.DAT     (root TCI index + metadata — the entry point)
CRYPTNAV/DATA/DATA/RNW/CCP/<REGION>/NAVnnnnn.DAT     (341 files for POL; cluster data)
CRYPTNAV/DATA/DATA/RNW/CCP/<REGION>/AEX/AEXnnnnn.DAT (per-cluster auxiliary data, optional —
    loaded only when config flag bRNWLoadAexData is set; same fileId numbering as NAV)
```

Region folders overlapping N6E2: `POL`, `CHS` (AT/CZ/SK), `EEU` (HU/UA/BY…),
`ELL` (Baltic), `INT`. A NAV file is a concatenation of **clusters** preceded by a
per-file build-metadata header block (see §11).

- A cluster starts at a **16KB-aligned** file offset.
- A cluster may span several 16KB blocks; continuation blocks have
  `listFlags == 0` and invalid ref coordinates (they are not cluster starts).
- **All list offsets are relative to the cluster start.**
- Clusters found in N6E2: POL 4266, CHS 2305, EEU 3214, ELL 1425, INT 29.

## 2. Cluster header (at start S)

| off | size | field | notes |
|-----|------|-------|-------|
| 0x00 | u16 | A | cluster ID. **Not unique per file** — 702/8257 files contain duplicates (checked on the EUR dataset); do not use as a key without the file id |
| 0x02 | u16 | flags | bit 0x40 → coordType 4 (rel24), else 3 (rel16) |
| 0x04 | u32 | C | sequence number. **Not monotonic within a file** (only ~3111/8257 files are strictly increasing); do not use for ordering/validation |
| 0x08 | s32 | refLon | PAU = deg·2³¹/180 |
| 0x0C | s32 | refLat | |
| 0x10 | s8 | shift | delta scale (0–12 observed) |
| 0x11 | u8 | pad | |
| 0x12 | u16 | ooff | outline offset (cluster-relative) |
| 0x14 | u16 | ocnt | outline point count |
| 0x16 | u16 | listFlags | descriptor presence bits (see §3) |
| 0x18 | u16 | annOff | cluster annotation offset |

Outline: `ocnt` points at S+ooff, each **4B** `{s16 dlon, s16 dlat}`
(coordType 3) or **6B** `{s24 dlon, s24 dlat}` (type 4);
point = ref + delta<<shift. (Cluster boundary polygon — not needed for roads.)

## 3. Descriptor sequence

Starts at `S + ooff + ptsize·ocnt`. Strict bit order 0–10 of `listFlags`:

| bit | content |
|-----|---------|
| 0   | AnnotList `{u16 off, u16 cnt}` (cluster annotations) |
| 1   | skip 4B |
| 2   | ClusterInfo ListDesc (adjacent cluster #1 — used for external refs) |
| 3   | ClusterInfo ListDesc (adjacent cluster #2) |
| 4   | Zerocell ListDesc |
| 5   | Onecell ListDesc |
| 6,7 | skip 4B each |
| 8   | Position ListDesc |
| 9,10| skip 4B each |

`ListDesc = {u16 listOffset, u16 count}`: jump to `listOffset`, read `count`
elements sequentially, restore stream position. (`rnw_tclListDesc<T>::bRead`.)

### 3a. ClusterInfo (bits 2/3) — adjacency / external refs (decoded)

`nav_tclClusterInfo` describes a **neighbouring cluster**. Decoded from
`nav_tclClusterInfo::bRead` @0x008910cc + `rnw_tclOutline::bRead` @0x00892d70 and
validated on the POL dataset (2634/2634 elements parse with plausible coordinates).

Each element is a **fixed 24-byte** record; the outline *points* are stored separately
at a cluster-relative offset (not inline), which is why `bRead` does a skip→read-status→
seek-back double pass:

```
+0x00  u32  clusterFileOffset   offset of the neighbour cluster within its NAV file
+0x04  u16  size                neighbour cluster size (bytes)
+0x06  u16  fileId              neighbour's NAV file id == the number in NAV<fileId>.DAT
+0x08  s32  refLon              outline reference position (PAU)
+0x0c  s32  refLat
+0x10  s8   shift               outline delta scale
+0x11  u8   ?                   (observed 0x05; subtracted into an internal count)
+0x12  u16  pointsOffset        cluster-relative offset of the outline point list
+0x14  u16  pointsCount         number of outline points
+0x16  u16  status              bit 0x1000 set -> coordType 4 (rel24) else 3 (rel16)
```

The `pointsOffset` point list holds `pointsCount × rnw_tclPositionInternal`
(coordType from the status bit — same encoding as §4/§7). In-memory the object is 50
bytes (`ClusterIDInternal` 12B incl. vptr, `CoordInfo` 16B, `Outline` 20B = ref pos +
point count, then `u16 status`).

**fileId == NAV filename number**: the fid values in ci lists match the `NAVnnnnn.DAT`
names exactly (e.g. a cluster in `NAV27284.DAT` references neighbours with fid 27284,
24905, 21825…). Interior clusters reference same-file neighbours (fid == own file);
boundary clusters reference neighbouring files. The runtime uses the ci1/ci2 lists to
load a neighbour cluster when it needs geometry beyond its own boundary. This is **not**
a road/node identifier: neither `rnw_tclOnecellInternal` nor
`rnw_tclZerocellInternal` carries a global ID (their onecell refs are
`rnw_tclLocalOneCellRef`, zerocell refs `rnw_tclDirectCellRef`, both local-indexed).
Cross-cluster node identity is by **position** — a boundary node is duplicated at the
same absolute coordinate in each adjacent cluster, so stitching across clusters is a
coordinate match, not an ID lookup. (An earlier theory that bit-15 DCR refs named a
neighbour's onecell was wrong; see the correction note in §5.)

> The runtime locates clusters via the CCP container index, **not** by scanning. The
> extractor's 16KB-aligned scan is a heuristic; the reference lon/lat in the header
> therefore doubles as a validation signal (see §9).

## 4. Position list (bit 8) — node positions

- Element: **4B** `{s16 dlon, s16 dlat}` (type 3) or **6B** `{s24, s24}`
  (type 4); position = ref + delta<<shift.
  (`rnw_tclPositionInternal::bRead` @0x00891824: type 1 = abs {s32,s32},
  type 2 = rel8 {s8,s8}·256 with NO ref added, types 3/4 as above.)
- **count == zerocell count; index == zerocell (node) index.** Verified:
  438/439 nodes within 30 m of MAP vertices (median 4.8 m).

## 5. Zerocell (bit 4) — nodes

Record: **6B** `{u16 f1, u16 listFlags, u16 offz}`; descriptors at `offz`:

| bit | content |
|-----|---------|
| 0   | AnnotList `{u16 off, u16 cnt}` |
| 1   | DirectCellRef ListDesc |

**DirectCellRef element = one u16 `v`** (`rnw_tclDirectCellRef::bRead`
@0x00892508: `idx = (v & 0x3FF) - 1`, flags stored raw):

- onecell index = `(v & 0x3FF) - 1` — **1-based**, 10 bits. Confirmed 1-based
  empirically: `raw & 0x3FF == onecellCount` occurs (impossible if 0-based), and
  the value is never 0.
- **bit 15** (`0x8000`): set → this zerocell is the road's **TO** (end) node,
  clear → the **FROM** (start) node. Confirmed against
  `rnw_tclBaseExtensionGenerate::u16AddFromAndToZerocell` @0x009167e8, which writes
  `from[onecell]=(v&0x3FF)` when bit 15 clear and `to[onecell]` when set.
  (Bits 13/14 are always 0 in the data.)

> **Correction (was documented as "bit 14 = FROM, bit 15 = external ref").** Both
> were wrong. The extractor originally tested bit 14 (`0x4000`) to pick FROM/TO — a
> bit that is always 0 — so `from_node` was never assigned and ~218k relative-shape
> roads collapsed to a single point. Testing bit 15 instead resolves them
> (POL: single-point 219,342 → 597; see §9). There is **no external/cross-cluster
> reference in the DCR list**: across POL+N6E1 every one of 24.4M refs has a
> local-range index. Cross-cluster node identity is by *position* (boundary nodes
> are duplicated at identical absolute coordinates in both clusters), not by ref.

`vAddCobZCToOC` @0x00890108 writes these into onecells:
FROM → onecell+0x28, TO → onecell+0x2A. Both FROM and TO refs are present in the
local cluster's zerocell DCR lists (each endpoint node has its own local zerocell).

## 6. Onecell (bit 5) — road segments

Record header: **12B** `{u32 hdr, u32 x, u16 listFlags, u16 offf}`;
descriptors at `offf` (bit order):

| bit | content |
|-----|---------|
| 0   | AnnotList `{u16 off, u16 cnt}` |
| 1   | shape ListDesc — **forced coordType 2** (see §7) |
| 2   | two **inline u16** upcell refs (8B total, NOT a ListDesc) |
| 3   | Downcells ListDesc |
| 4   | Overlaps ListDesc |
| 5   | shape ListDesc — cluster coordType (3/4), absolute |

Bits 1 and 5 are **mutually exclusive** (`bRead` @0x00892978: bit 5 is read
only when bit 1 is absent; bit 1 forces a type-2 CoordInfo copy).

`hdr` bits (verified via the `rnw_tclOnecellInternal` accessors):

| bit(s) | field | accessor |
|--------|-------|----------|
| 0–2    | roadClass (`rc`) | `u8GetRoadClass` @0x008888ac = `hdr & 7` |
| 3      | gateway | `bIsGateway` |
| 4–6    | networkClass (`nc`) | `u8GetNetworkClass` @0x008888b8 = `(hdr>>4)&7` |
| 8–11   | roadType (`rt`) | `u8GetRoadType` @0x008888c8 = `(hdr>>8)&0xF` (valid 0–10) |
| 13     | link (ramp/connecting) | `bIsLink` |
| 15     | secundary | `bIsSecundary` |
| 30     | freeway | `bIsFreeway` @0x00888998 = `(hdr>>30)&1` |

Other onecell predicates exist (`bIsFerry/bIsTunnel/bIsBridge/bIsRoundAbout/
bIsOnewayTo/bIsOnewayFrom/bIsParallel/bIsLongRamp/bIsRestricted/…`) — not yet
bit-mapped or emitted. Observed (rc,nc) pairs concentrate on (5,7), (6,7), (3,3),
(2,2); the rest are rare. Valid `nc` values are {0,1,2,3,7}
(`bIsTpNavNetClassValid` @0x00b5a474).

### 6a. Road class → OSM `highway=*`

The runtime classifies roads for rendering via
`rnw_tclMAPConverter::enConvertRoadSubattrDisplayClass(rc, nc)` @0x00888b14, a
2-D lookup producing an ordered **display class** (lower = more important):

| rc \ nc | 0 | 1 | 2 | 3 | 7 | any |
|---------|---|---|---|---|---|-----|
| 0, 1    | 2 | 6 | 7 | 8 | 9 |     |
| 2       |   |   |   |   |   | 9   |
| 3       |   |   |   |   |   | 10  |
| 4       |   |   |   |   |   | 11  |
| 5, 7    |   |   |   |   |   | 12  |
| 6       |   |   |   |   |   | 13  |

(`rc` is the dominant axis; `nc` only subdivides `rc` 0/1.) Direction is confirmed
by the data: **dc=2 is the only class carrying the freeway bit** (all freeway
onecells are rc=0,nc=0) and its joined roads are motorway interchanges
(`WĘZEŁ …`); dc=12/13 are ~84% of all POL onecells (local streets). `rnw_join_rs`
maps display class → OSM `highway` monotonically, and appends `_link` for the major
classes when `bIsLink` is set:

| dc | 2 | 6 | 7/8 | 9/10 | 11 | 12 | 13 |
|----|---|---|-----|------|----|----|----|
| highway | motorway | trunk | primary | secondary | tertiary | residential | unclassified |

The exact userdef rendering-class table (`dap_tclRoadClassConvr`, an 8×8 nibble
matrix loaded from app config via `bSetRoadClassConv`) is **not** in the dataset, so
the middle tiers (primary/secondary/tertiary) are assigned by ordinal position rather
than a named source table. The joiner emits `highway` plus the raw `rn_class`,
`rn_netclass`, `rn_roadtype`, `rn_link`, `rn_sec`, `rn_freeway` on every matched road.

## 7. Geometry decoding rules (validated vs MAP)

MAP line written by the nav engine = **`[fromNode] + shapePts + [toNode]`**
(`bConvertPositionList` @0x008896a4). A MAP road may chain several onecells
(MAP merges consecutive same-class segments), so a MAP line can be longer
than any single RNW road.

**bit-5 roads (rel16/rel24, absolute):** each shape point is independent:
`pt = ref + delta<<shift`. Validated: 46/46 named Kraków roads within
median ~1 m of MAP vertices.

**bit-1 roads (rel8, "reduced"):** each on-disk point is `{s8 dlon, s8 dlat}`,
`delta = s8·256` PAU, **no ref added at read time** (type 2). The cluster
reader then converts them (`vCoordReduced2Absolute` @0x00892638, only when
onecell coordType==2): **each point independently = fromNode + delta**.
Practical placement using the locally-known TO node:
`pt_i = toNode + (d_i − d_last)` (last shape point coincides with toNode).
Validated: 24/24 rel8 roads within 16 m (median <5 m).

Both node endpoints are exact once the Position list is decoded, so a full
RNW road polyline = `[fromNode?] + shapePts + [toNode]` reproduces the MAP
line point-for-point (verified: KOSOCICKA case, MAP 6 pts = 4 shape + 2 nodes).

## 8. Names and annotations

Onecell AnnotList elements (cluster-relative `off`, `cnt` entries):

```
{u16 size, u16 type, payload[size-4]}
```

Type **0x3C** = name (`nav_tclAnnotName::bRead` @0x0088f740); payload =
`{u16 textOff}` — cluster-relative offset of the string:

```
{u8 nVar, per variant: {u8 flag(≈0xA7), u8 len}, then all UTF-8 texts in order}
```

(headers first, THEN texts — not interleaved). Variant 0 = local script,
later variants = transliterations. Example (ULICA KOBIERZYŃSKA):
`02 A7 13 A7 12 "ULICA KOBIERZYŃSKA" "ULICA KOBIERZYNSKA"`.

### 8a. Full annotation type dispatch (decoded)

The same `nav_tclAnnotList` reader backs the AnnotList of **clusters, onecells, zerocells and the
file header** (callers: `rnw_tclClusterInternal::bRead` @0x008906ec, `rnw_tclOnecellInternal::bReadAnnotations`
@0x008927a0, `rnw_tclZerocellInternal::bRead` @0x00894734, `nav_tclFileHeader::bRead` @0x009165f4). Dispatch is
`nav_tclAnnotList::bRead` @0x0088fb90. Frame = `{u16 size (incl. 4B header), u16 type}`; **bit 15 of `type`
is a flag**, code = `type & 0x7fff`:

| code | name            | reader @   | in-mem B | meaning                        |
|------|-----------------|------------|----------|--------------------------------|
| 0x02 | DistanceMatrix  | 0x0088f5a8 | 8        | routing cost matrix            |
| 0x17 | GenTimeDist     | 0x0088f41c | 20       | generalized time-distance (4)  |
| 0x1b | RealLength      | 0x0088f4e8 | 4        | signed real length (m)         |
| 0x1d | State           | 0x0088f518 | 4        | road state / status            |
| 0x3c | Name            | 0x0088f740 | 8        | name ref (only if loader bit1) |
| 0x3d | RoadNumber      | 0x0088f9f0 | 8        | road number                    |
| 0x4f | RoadNumberDir   | 0x0088f81c | 16       | directional road number        |
| 0x65 | SubNational     | 0x0088f58c | 6        | sub-national flag              |
| 0x74 | DirectionFlowVT | 0x0088f374 | 6        | direction-of-flow / VT         |
| other| Unknown         | 0x0088f008 | 2        | skipped (`size-4` bytes)       |

**Routing-cost payload layouts (decoded):**
- **DistanceMatrix (0x02):** `{u8 rows, u8 cols, u8[rows*cols]}` — raw distance matrix, memcpy'd verbatim.
- **GenTimeDist (0x17):** `{u16 shift, u16 v0..v3}`; each stored metric = `vi << shift` (4 values, one scale).
- **RealLength (0x1b):** `{s16}` — signed metres.

> Note: this is the **NAV cluster** annotation system. The separate **AEX "extern annotation"** files (§13)
> are a different on-disk format with their own type-code space and are parsed client-side, *not* by this reader.

## 9. Extraction + join pipeline

```
rnw_extract_rs [CCP_DIR] [OUT.jsonl] [-b W,S,E,N|none]
rnw_join_rs    RNW.jsonl MAP_L2.osm OUT.osm # OSM XML in and out; ~10 s for N6E2 L2
```

`-b` sets the geographic sanity filter (degrees) for the 16KB-aligned cluster scan.
Default `-30,30,60,75` covers the whole EUR dataset (Iceland..Turkey, Morocco..N.
Scandinavia). The old hardcoded N6E2 box silently dropped ~88% of clusters (141k of
161k); with the default box the full dataset yields **22.3M roads** (~71 s, I/O bound)
vs 1.83M before. `none` disables the filter and is for diagnostics only — without it,
padding/continuation blocks that pass the structural checks are accepted and emit
garbage multi-kilobyte shape lists (the ref lon/lat is what normally rejects them).

Zero-dependency Rust (std only); build with `cargo build --release` in each
project directory (binaries land in the shared cargo target dir
`/home/marek/Ext/.cargo_cache/release/`).

`rnw_join_rs` reads the OSM XML produced by `map2osm_rs`, enriches the road
ways (`tm:layer == "road"`) with RNW names/attributes, and writes a new OSM
XML file; all other elements pass through unchanged.

Extractor geometry (from/to fix): each road is `[fromNode] + shapePts + [toNode]`,
with from/to chosen by DCR bit 15 (§5). A prior build tested the always-zero bit 14,
so `from_node` was never assigned and ~218k relative-shape roads emitted as a single
point. After the fix (POL): single-point/empty roads **219,342 → 597** (−99.7 %),
roads with usable geometry in the join index **462k → 681k**, and the named-road MAP
cross-check improved **94.6 % → 96.7 %**. The remaining 597 have neither endpoint node
in the local cluster and are dropped from the join index (`pts.len() >= 2`).

Results on N6E2 L2 (70,504 MAP road ways):
- 1,830,749 RNW roads extracted from 8,257 files; 703,145 named (38%).
- **6,844 previously-unnamed MAP roads gained names**. All matched roads also get
  the RNW class attributes (`rn_class/rn_netclass/rn_roadtype/rn_link/rn_sec/
  rn_freeway`) and a derived OSM `highway=*` tag (see §6a).
- Named-road cross-check: 12,031/13,795 (87%) component names agree with the
  existing MAP name. Disagreements are concentrated in Budapest
  embankments/bridges where parallel named sections lie <30 m apart
  (genuinely ambiguous), plus sparse-RNW areas (HU/UA).
- Join method: a RNW road is a *component* of a MAP road when both its
  endpoints lie ≤30 m on the MAP polyline and ≥80 % of its points are
  ≤30 m from it; names/attributes are combined over all components.

Output format notes:
- JSONL: one road per line, keys `f,k,name,alt,rc,nc,rt,link,sec,fw,pts`;
  non-ASCII names as raw UTF-8, floats in native Rust display; `pts` = `[lon, lat]`
  degree pairs or null (no geometry).
- OSM output uses the canonical map2osm layout: one line per `<node>`; a
  `<way>` keeps all its `<nd>` on one line and all its `<tag>` on one line
  before `</way>`. Verified on N6E2 L2 (3,762,671 elements) via
  `osmium cat → PBF`.

Caveats:
- Both FROM and TO nodes are resolved from the **local** cluster's zerocell DCR lists
  (bit 15 picks the direction, §5). The extractor emits `[fromNode] + shapePts +
  [toNode]`. The join is distance-based, so an occasional missing endpoint does not
  break matching.
- A road is emitted single-point (or empty) only when **neither** endpoint node has a
  local zerocell ref — a genuine degenerate case, ~0.09 % of POL roads (597 of
  681,682). These are excluded from the join index (`pts.len() >= 2`). No multi-file /
  cross-cluster recovery is needed for the common case: there are no external DCR refs,
  and boundary nodes are duplicated by position (§3a/§5).
- Cluster IDs (A) are not unique per file and sequence numbers (C) are not
  monotonic within a file — neither is safe as a key/ordering signal.

## 10. Dead ends (for the record)

- `N6E210I.TCI` cluster-list sections are zero-filled on disk — TCI is not a
  usable RNW↔MAP join path.
- The "UN" magic seen in NAV file headers is just fileId 20053 as u16 LE.
- MAP annotation layout ({u8 size,u8 type}) differs from RNW annotations
  ({u16 size,u16 type}) — mixing them up silently yields zero names.

## 11. Generating your own `.RNW` (NAV/AEX) — write-side status

The firmware contains only the **reader** (`DAPIAPP.OUT`); the build tools are absent, so a
byte-exact write layout is inferred from the reader + data. A region is three things:
`NAV_ROOT.DAT` (root index), `NAVnnnnn.DAT` (clusters), and optional `AEX/AEXnnnnn.DAT`.

### Container / how clusters are located (decoded this pass)

- **`NAV_ROOT.DAT`** = root of the cluster tree. Layout: a ~0x20-byte numeric header, then a
  string table (the 8 compass-sector names in EN/DE/FR — `EAST`, `NORTH-EAST`, …, version
  `2021.1`, and a country list), then the **TCI** (Tile Cluster Index).
- **TCI** is a set of *tiles*; each tile carries `#primcl` (primary-cluster count), `#cl`
  (cluster count) and a list of **8-byte cluster entries**:
  ```
  +0x00  u32  fileOffset     offset of the cluster within its NAV file
  +0x04  u16  length         cluster size in bytes
  +0x06  u16  fileId         which NAVnnnnn.DAT (== filename number)
  ```
  (`dap_map_tclTCIClusterId::bRead` @0x008e01ec.) Loaded by
  `u16LoadClusterIdListAndStoreInQ` @0x008de974 / `u16LoadClusterIndexTile` @0x008df4a0.
- **Cluster load path:** `u16ReadCluster` @0x0088670c → `u16LoadCluster` @0x0090add4
  (fileId→filename via `vFileId2Name`, then read `{offset,length}` bytes) →
  `rnw_tclClusterInternal::bRead(..., flags=0x3060313, ...)` → `u16PatchCluster` (post-load
  fixup). So a cluster is addressed by **(NAV file, offset, length)** from the TCI — **not** by
  scanning. The extractor's 16KB-aligned scan (§1) is a read-only heuristic that works because
  non-cluster blocks fail the reference-coordinate check.
- A `NAVnnnnn.DAT` may begin with a **build-metadata block** before its first cluster (e.g.
  `NAV00001.DAT`: first cluster at 0x4000; block 0 holds the source filename
  `databases//00001.wrk`, copyright, build date, project name and a shell-env dump
  `REGION_CODE=POL PROFILE_CODE=CCP`). Analogous to the MAP info-string region — display only.

### Cluster format — fully known (writable)

Every field is now mapped via `rnw_tclClusterInternal::bRead` @0x008906ec (see §2/§3):
header `A`(u16, **skipped by reader**) / `B`(u16 flags, bit 6 → coordType) / `C`(u32, **skipped
by reader**), the Outline (`refLon/refLat/shift/?/ooff/ocnt` + points), then `listFlags` +
`annOffset`, then the descriptor sequence. All 11 listFlags bits are now accounted for:

| bit | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 |
|-----|---|---|---|---|---|---|---|---|---|---|----|
| reader | AnnotList | skip{off,cnt} | ci1 (ClusterInfo) | ci2 (ClusterInfo) | zerocells | onecells | skip{off,cnt} | skip{off,cnt} | positions | skip{off,cnt} | skip{off,cnt} |

- The **skip** bits (1,6,7,9,10) are read as a 4-byte `{u16 off,u16 cnt}` and the payload is
  **ignored** by the reader → a writer emits any 4 bytes (e.g. `0,0`) when the bit is set.
  In POL data only bits 0,1,2,3,4,5,8,10 are ever set; 6,7,9 never appear.
- **Onecell** = `{u32 hdr, u32 x, u16 listFlags, u16 offf}`; the `x` u32 is read into the struct
  but **never used** in `bRead` (reserved). The onecell's *own* listFlags: bit 1 = shape points
  (type-2 rel8), bit 5 = shape points (type-3/4 absolute) — mutually exclusive; then upcells /
  downcells / overlaps. In-memory size 0x30 B (`ListDesc<Onecell>::bRead` @0x008904a8).
- **Zerocell** = `{u16 f1, u16 listFlags, u16 offz}`; `f1` is a node type/flags field (values
  `0`/`1` common, occasional `0x22xx`) — not fully mapped (§5).

### Still not fully pinned (all avoidable for a minimal working file)

- **`A` (u16@0) / `C` (u32@4):** the reader skips both → write `0` or copy from a reference file.
- **Onecell `x` (u32@+4):** read but unused in `bRead` → reserved; write `0`/copy.
- **Zerocell `f1`:** node type/flags, partially understood; write `0` for simple nodes.
- **`u16PatchCluster` post-load fixup:** resolves cross-cluster references by position after a
  load. Avoid the need entirely by keeping all geometry in **one cluster** (no boundaries).
- **TCI tile/sector partitioning:** the full geographic→tile→cluster mapping. For a minimal
  dataset emit a single tile listing one cluster that covers the whole area.
- **AEX content:** optional — omit and leave `bRNWLoadAexData` unset.
- **Checksum/CRC:** none observed in any header, cluster, or TCI entry.

### Minimal viable RNW dataset (loadable for geometry/routing)

One `NAVnnnnn.DAT` containing a **single cluster** with all nodes (zerocells), roads (onecells),
and the position list for the area, plus a minimal `NAV_ROOT.DAT` whose TCI has one tile with one
8-byte entry `{offset, length, fileId}` pointing at that cluster. Leave the cluster's ci1/ci2
lists empty (no neighbours) so no cross-cluster patching is triggered, and omit AEX. This is
enough for the reader to load and route within the region; it will not reproduce the original
multi-cluster tiling, which is not needed for a conversion target.

## 12. Runtime RNW→MAP conversion ("FastMap") — how the two formats meet at run time

An earlier note (in the chat record) said "RNW → MAP in the build pipeline." That was an
**unverified inference and is wrong**: RNW does **not** contain enough to build a full `.MAP`
(no water / polygons / POIs — see below). What is actually true, and what `DAPIAPP.OUT` shows, is a
**live run-time conversion** used by the map renderer.

### The map renderer loads blocks from several sources, chosen by block *type*

`dap_map_tclWorker::u16ProcessLoadBlocks` @0x00849894 (in `components/dapi/map/Worker.cpp`) receives a
"load blocks" request (`fastmapfi_tclMsgLoadBlocksMethodStart`) carrying a list of block IDs. Each ID
has a **type**, and the type selects the *source*:

| block type | source | code path |
|-----------:|--------|-----------|
| 1 / 4 / 6  | pre-built `.MAP` files on disk | `dap_map_tclDataContext::u16Load` (via `dap_tclDataAccess`) |
| **2**      | **RNW clusters, converted live** | `u16LoadRnw` @0x00846054 → RPC → RNW worker |
| 7          | "LID" (another source) | `u16LoadLid` |

So the rendered map is a **composite**: background layers (water, areas, POIs) come from the
pre-built `.MAP` files, while a road layer can be served **directly from the RNW road network**.

### The type-2 path (RNW → MAP blocks), end to end

1. `dap_map_tclWorker::u16LoadRnw` @0x00846054 (MAP component) collects the requested cluster IDs and
   sends a `dap_rnwfi_tclMsgGetMapBlocksMethodStart` RPC to the RNW worker, with a write buffer.
2. `rnw_tclAccessMAPWorker::u16ProcessGetBlocks` @0x00888100 (RNW component) receives it; for each
   cluster it calls `u16ReadCluster` (§11 load path) then
   `rnw_tclMAPConverter::u16ConvertCluster` @0x0088abac, writing the result into the MAP worker's
   buffer, and sends it back.
3. The MAP worker consumes those blocks exactly like any other map block.

### What a converted cluster contains (and what RNW is missing)

`u16ConvertCluster` @0x0088abac emits only:
- a start block (`bWriteStartBlock`),
- the **onecells** — `u16ConvertOnecellList` → `bConvertOnecell` → `bConvertRoadAttribute` →
  `bConvertRoadAttrSubattrList` → `enConvertRoadSubattrDisplayClass` (§6a),
- the strings/names — `rnw_tclMapStringConverter::bConvertAllStrings`,
- a metadata sequence — `bWriteMetaDataSeq`.

There is **no polygon, water, or POI output** — because RNW carries none of those. This confirms
RNW = road network only, and that the on-disk `.MAP` (which *does* have polygons/POIs) is **not**
generated from RNW. The two are independent products of a common master source; the run-time
converter exists so the drawn roads match the routed network exactly (and to avoid storing the road
layer twice). The `fastmapfi_*` / `dap_map_tclMap2FastMapConverter` naming indicates this is the
"FastMap" rendering path.

> Practical consequence for a converter: you do **not** need RNW to reproduce `.MAP`, and you do not
> need the run-time converter at all if you generate both `.MAP` and `.RNW` from your own OSM source —
> they are sibling outputs, not a pipeline.

## 13. AEX files — per-cluster annotation export (decoded)

`AEX/AEXnnnnn.DAT` sits alongside `NAVnnnnn.DAT` with the **same file id** (`nnnnn`). It holds the
per-cluster **"extern annotation"** data — extra per-road/per-node annotation records that the NAV
clusters themselves do not carry inline. The embedded metadata literally reads `(c) 2006 Blaupunkt GmbH,
Hildesheim`, a build timestamp, and the tool tag **`TPNAV_ANNEXPORT`**.

It is **optional** and loaded **on demand**: only when (a) the config flag `bRNWLoadAexData` is set
(registry key `APP_CONFIG/RNW/LOAD_AEX_DATA`; stored in `rnw_tclManager` at `this+0x4cc`, passed to the
worker as `this+0x2fc`) **and** (b) a client requests that cluster's annotations over RPC. The loader is
`rnw_tclAccessWorker::vProcessClusterInfo` @0x00908648 — on an `ANNOTATION`-type request it builds
`<region>/AEX/AEXnnnnn.DAT` (`strcat(path,"AEX/")` + `vFileId2Name(name, fileId, 1)`) and copies the
requested `[off,size)` slice into shared memory via `u16LoadDataBlockViaCache` (trace:
`"LOAD EXTERN ANNOTATION (%s%s)"`).

> **DAPIAPP only ships the raw bytes** — it does *not* parse them. The consumer is **`procmapengine.out`**
> (the map engine; it also carries the route-calc + `GlobalAnnotationTables` types) — see §13.3. The
> structure below was recovered from the data and validated against all POL AEX files, since procmapengine's
> own byte parser is not surfaced as named functions in Ghidra.

### 13.1 File container & record wrapper (validated across POL: index tiles `[recOffset, fileSize)` exactly)

```
+0x00  u16  recOffset      offset of the first record  == 0x64 + recordCount*12   (verified 3/3 files)
+0x02  u16  fileId         == the number in AEXnnnnn.DAT
+0x04  u32  fileSize       total file size in bytes        (verified == actual size)
+0x08  u16  metaEnd        = 0x54, end of the fixed metadata block (constant)
+0x0a  u16  idxStart       = 0x64, offset of the first index entry (constant)
+0x0c  u32  recordCount    number of records / index entries (scales with size: 1..113 observed)

[0x10 .. 0x54)  metadata strings — 3 NUL-terminated ASCII, fixed length:
                 "(c) 2006 Blaupunkt GmbH, Hildesheim", "<build timestamp>", "TPNAV_ANNEXPORT"
[0x54 .. 0x64)  a fixed 12-byte block that references the three string offsets (0x10/0x34/0x44)

[0x64 .. recOffset)   index: recordCount × 12-byte entries, tiling [recOffset, fileSize) contiguously:
                  +0x00  u32  key        = recordIndex * 0x4000  (observed 0x4000,0x8000,0x10000,...; a per-record id)
                 +0x04  u32  recordOffset
                 +0x08  u32  recordLength      (recordOffset[i]+recordLength[i] == recordOffset[i+1])

[recOffset .. fileSize)   records: variable length (2 KB up to ~9 KB observed). Each record = a 16-byte
                   wrapper header + an index table + the sub-records it points at:

                   +0x00  u16  totalLen      == record length (== index recordLength)
                   +0x02  u16  f2            = 0x20 in all samples (count of index entries, or a type tag)
                   +0x04  u16  f3            = 0x06 constant
                   +0x06  u16  f4            = 0x0a constant
                   +0x08  u16  f5            per-record offset (0x40 in sample)
                   +0x0a  u16  f6            = 0x01 constant
                   +0x0c  u16  f7            per-record offset (0x18c in sample)
                   +0x0e  u16  f8            = 0x01 constant

                   +0x10   index table: N × 6-byte entries {u16 id, u16 offset, u16 const=1}; ids
                            increment from 0x02 (0x02,0x03,0x04,...); each `offset` → one sub-record
                   ...     sub-records at those offsets; each begins with a type code in AEX's OWN space
                            (e.g. 0x46) — NOT the nav_tclAnnot* codes of §8a
```

### 13.2 Record structure (fully decoded, validated across all POL AEX)

A record is a self-describing blob: an 8-field header, an index table, and the sub-records it points to.

```
record:
+0x00 u16 totalLen          == record length
+0x02 u16 f2                = 0x20 in all samples
+0x04 u16 f3                = 0x06 constant
+0x06 u16 f4                = 0x0a constant
+0x08 u16 f5                per-record offset (into/after the index table)
+0x0a u16 f6                = 0x01 constant
+0x0c u16 f7                per-record offset (== index-table end + 2 in samples)
+0x0e u16 f8                = 0x01 constant

+0x10  index table: N × 6-byte {u16 id, u16 offset, u16 const=1}
                    ids increment from 0x02 (0x02,0x03,…); each `offset` → one sub-record
                    (rec0 example: 63 entries, ids 0x02..0x40)

sub-record (at each index offset):
+0x00 u16 len               == this sub-record length
+0x02 u16 type              = 0x46   (single annotation category — see §13.3)
+0x04 u16 count             number of entries that follow
+0x06 count × 6-byte entry: {u8 a, u8 b, u16 c, u16 d}
```

**Validation:** across every POL AEX file, all **1,980,883** sub-records satisfy `len == 6 + count*6` with
zero exceptions, and **every** one carries `type = 0x46`. Each AEX record is one **speed-limit table** of
per-segment entries `{a,b,c,d}` (field mapping below).

**What the annotation data actually is (decoded from DAPIAPP's fi_tcl framework):** in memory an annotation
table is the tagged union `fi_tcl_GlobalAnnotationTablesUnion = { vtable, e8_GlobalAnnotationCategory
category, payload ptr }` (ctor @0x00da66cc; `oRead` @0x00b258f0). The category enum has **7 values**
(`oRead` switch):

```
0 Reserved   1 TrafficSense   2 SpeedFactors      3 SpeedLimits
4 RequiredPermission   5 PrefixState   6 LanguageDesc
```

Each non-reserved category is a *list* of records. The SpeedLimits (3) branch, decoded fully:

```
SpeedLimitsList            = vector<SpeedLimitsCountry>
  SpeedLimitsCountry       = { e16_ISOCountryCode, vector<SpeedLimitRoadAreaType> }
    SpeedLimitRoadAreaType = { e8_RoadTypeDesc, vector<SpeedLimit> }      (leaf list)
```

i.e. *per country → per road-area-type → speed limits*. So the AEX "extern annotation" payload is
**road-attribute annotation data** (speed limits, traffic sense, speed factors, permissions, …).

**Per-entry field mapping `{a,b,c,d}` (decoded from `fi_tcl_SpeedLimit::oRead` @0x00ae78a4 + data):**

 | field | type  | meaning                                                        | evidence |
 | ---    | ---   | -------------------------------------------------------------- | -------- |
 | `a`    | u8    | **direction-of-travel selector** — `3`=undifferentiated (both directions, the default), `1` & `2`=the two travel directions, each with its own limit. Only values 1/2/3 occur. | see "Direction of travel" below |
 | `b`    | u8    | **speed-limit value, km/h** (50/90/40/70/60/30/80 …)           | `oRead` reads a `uchar`; data peaks at exact standard speeds |
 | `c`    | u16   | **start offset** of the speed zone along the road, in **metres** (≈0 in 97%) | empirical: min 0, max 882, 97% zero |
 | `d`    | u16   | **length** of the speed zone, in **metres** — the zone spans `[c, c+d]`        | a row with `d < c` rules out end-offset; zones chain contiguously |

 **Direction of travel (`a`) — decoded from the data (3.73M road-entries across all POL AEX):**
 - `a` takes only values **1, 2, 3**. **90% of roads carry a single `a=3`** (the undifferentiated limit that
   applies to the whole road). Only ~3.4% split by direction.
 - `a=1` and `a=2` are a **symmetric pair**: they occur in near-identical counts (roads containing `a=1` =
   257,704 vs `a=2` = 257,496; singleton sets `{1}`=131,957 vs `{2}`=131,770). Set composition:
   `{3}` 3.34M · `{1}` 132k · `{2}` 132k · `{1,2}` 93k · `{1,2,3}` 32k · `{1,3}/{2,3}` ~0.3k (noise).
 - Of the roads that have **both** `a=1` and `a=2`, **89.9% give them different speeds**, and the asymmetry is
   **balanced** (`a1<a2` = 56,910 vs `a1>a2` = 56,040, ≈50/50). A day/night (time-frame) reading would show a
   strong one-way skew (day ≥ night almost always); the 50/50 balance rules that out and matches two travel
   directions whose limits differ only because of local geometry.
 - In ~21% of multi-`a` roads one value covers the full span while the other is a short sub-segment
   (90% of those, the full-span one is `a=3`) — i.e. a default whole-road limit plus a short directional
   exception on one side.
 - **This empirically rules out `a` = road class:** a single road entry carries *multiple* `a` values at once
   (e.g. `{1,2,3}`), which is impossible for a per-road class attribute.

 The in-memory leaf is `fi_tcl_SpeedLimit = { e8_RoadClassCode, uchar speed, fi_tcl_SpeedLimitStatus }`
 with `SpeedLimitStatus = { e8_SpeedUnit, e8_SpeedType, b8_SpeedLimitStatus }` (copy-ctor @0x00a9d280). The AEX
 file is a **compact export** of per-zone speed limits: `{directionSelector, speed, startOffset(m), length(m)}`.
 Note the struct's first field is *named* `e8_RoadClassCode` (which is why an early reading guessed "road class"),
 but the data shows the AEX `a` behaves as a **direction selector**, not a class — so AEX does not map 1:1 onto
 the fi_tcl leaf, and `c`/`d` are the on-road range in metres, *not* the status enum.

**Unit cross-check (metres confirmed):** AEX entries are finer-grained than the topological roads
`rnw_extract_rs` pulls from the same NAV clusters (~6–7× more, each ~6–10× shorter), so per-road lengths don't
match 1:1 — but summing per cluster (which cancels the granularity) gives the same order of magnitude in both:

| cluster | extracted roads total | AEX `Σ(c+d)` total | ratio AEX/ext |
| --- | --- | --- | --- |
| 20053 | 7600 km | 4020 km | 0.53 |
| 20003 | 778 km  | 410 km  | 0.53 |

The consistent ~0.53 (≈half the network carries speed-limit zones) — and *not* a 10×/100× shift — confirms
`c`/`d` are **metres**, matching the documented `RealLength` (`s16`, metres, §8a).

**1:1 spot-check (length fingerprint):** AEX carries no road ID, so an entry is matched to a real road by its
total length `max(c+d)` against the road's geometric length. For *distinctive-length* roads the match is unique
and tight — e.g. **JAGODZIN** (a 4338 m highway, class 5) ↔ an AEX entry of exactly **4340 m**, one zone
`120 km/h @ 0–4340` (0.04% off); also ULICA GNIEWOMIER 5240↔5219 m `@120`, BARTOSZÓW 3263↔3275 m
(multi-zone `90/50/40`). Agreement to <0.5% on a several-km road makes coincidence implausible, and the zone
speeds are sensible (highways at 120). *Caveat:* this is length-based, not an explicit ID link — the exact
entry↔road mapping lives in procmapengine's stripped parser — so it confirms the unit and correspondence, not
byte-identical per-road identity.

**Correction:** `0x46` (=70) is **not** an `e8_GlobalAnnotationCategory` value — that enum is only 0–6 and
the fi_tcl wire format contains no 0x46 at all. The AEX file is a separate compact encoding; `0x46` is its
 constant sub-record tag. (Two earlier notes were wrong and are now corrected: `a` does **not** "select a
 category", and it is **not** the road class — the data shows `a` is a **direction-of-travel selector**;
 and `b` is not an index but the km/h value.)

### 13.3 Consumer & loading path

- **Consumer = `procmapengine.out`** (the map engine; a hybrid that also does route-calc — it carries
  `fi_tcl_GlobalAnnotationTablesUnion`, `fi_tcl_e8_GlobalAnnotationCategory`, `NavRouteCalcProperty`,
  `DynamicRouteCalcMode`). `procbaselx_out.out` is only a process supervisor (no annotation symbols) and
  `PROCNAV.OUT` has none either.
- **Loader = `DAPIAPP.OUT`** `rnw_tclAccessWorker::vProcessClusterInfo` @0x00908648: on an
  `ANNOTATION`-type request + AEX flag on (`this+0x2fc`) + valid offset, it builds
  `<region>/AEX/AEXnnnnn.DAT`, copies `[off,size)` into shared memory via `u16LoadDataBlockViaCache`, and
  returns it. **It ships raw bytes only — no parsing.**
- The `fi_tcl_*Annotation*` symbols are RPC-marshalling stubs (unresolved in the `.out`/`.so`s inspected);
  the AEX raw bytes are parsed by procmapengine's own code, which Ghidra has not yet surfaced as named
  functions in `procmapengine.out`.

### 13.4 What is / is not pinned

- **Pinned (byte-level):** container; `key = recordIndex*0x4000`; record header fields (constants
  confirmed); the `{id, offset, 1}` index table; and the sub-record layout `{len, type=0x46, count,
  count×{u8 a, u8 b, u16 c, u16 d}}` — validated on 1.98M records. Consumer + loader identified.
- **Pinned (semantics):** `a` = **direction-of-travel selector** (`3`=both/default, `1`/`2`=the two directions) —
   proven from the data across 3.73M entries: only values 1/2/3 occur, `a=1`/`a=2` are a near-equal symmetric
   pair present per-road, 89.9% of split roads give them different speeds with a balanced (≈50/50) asymmetry
   (which rules out a day/night time reading), and one entry can carry several `a` at once (which rules out a
   per-road class). `b` = speed limit km/h (from `fi_tcl_SpeedLimit::oRead` @0x00ae78a4 + data). `c` = zone start
   offset (m), `d` = zone length (m) — `d` is a length, not an end offset (a `d<c` row is impossible otherwise);
   unit **confirmed as metres** by the per-cluster total-km cross-check above.
 - **Not pinned (direction polarity):** which physical direction `a=1` vs `a=2` maps to. *Working convention for
    generation:* `a=1` = travel **from→to** (the stored geometry order, `pts[0]→pts[-1]`, main.rs:364), `a=2` =
    the reverse — this matches how the RNW topology orders edges and is the standard in map formats, but it is
    **not byte-proven**. Ground truth was attempted and is currently blocked: AEX has no road ID (only length-based
    matching to NAV roads), OSM carries no `maxspeed`/directional tags on these POL roads, and procmapengine's
    parser is stripped. To confirm, check one split road on a map that shows per-direction limits — e.g. **ULICA
    LIPOWA** (rc=3, 3470 m; dir-a=1 = uniform 120, dir-a=2 = 90/120), FROM-node `53.79923,20.29793`, TO-node
    `53.82805,20.31102`: if the from→to leg is the uniform-120 one, the convention holds. Also not pinned: the roles
    of header `f2..f8`; and whether other `type` codes besides `0x46` exist in other regions (POL is 100% `0x46`).
  A byte-exact per-field confirmation needs procmapengine's AEX parser, which is not symbolized (`nm` shows zero
  annotation functions; the `fi_tcl_*Annotation*` strings are generic-serializer type names and the parser code
  is stripped). The fi_tcl `oRead`s used above come from DAPIAPP (same framework, fully analyzed) as a stand-in.

> Practical consequence: **omit AEX** (leave `bRNWLoadAexData` off) and the network loads + routes via the
> runtime RNW→MAP conversion (§12). The AEX "extern annotation" data is additive. To emit a byte-perfect
> AEX you can already reproduce the container + record + sub-record layout (structure is fully known); only
> the per-entry *values* require knowing what category-`0x46` data the target expects.
