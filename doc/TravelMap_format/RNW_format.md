# RNW (Road Network) Database Format — Bosch TravelMap / Nissan LCN2KAI

Reverse engineered from `DAPIAPP.OUT` (Ghidra) and validated against the MAP
converter output (`new_file_format_eng.md`) for region **N6E2**
(18.00–36.00°E, 47.25–56.70°N: eastern Poland, Slovakia, NE Hungary, western
Ukraine/Belarus, Baltic coast).

Tooling: `rnw_extract_rs` (extractor) and `rnw_join_rs` (join onto MAP OSM XML) —
zero-dependency Rust; build with `cargo build --release`.

## 1. File organization

```
CRYPTNAV/DATA/DATA/RNW/CCP/<REGION>/NAVnnnnn.DAT     (341 files for POL)
CRYPTNAV/DATA/DATA/RNW/CCP/<REGION>/AEX/AEXnnnnn.DAT (Blaupunkt annotation
    exports, "TPNAV_ANNEXPORT" — NOT cluster payloads, ignore)
```

Region folders overlapping N6E2: `POL`, `CHS` (AT/CZ/SK), `EEU` (HU/UA/BY…),
`ELL` (Baltic), `INT`. A NAV file is a concatenation of **clusters**.

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

Other annotation types appear in onecell lists (e.g. 0x17) — not yet mapped.

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
