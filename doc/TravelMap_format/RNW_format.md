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

### 3a. ClusterInfo (bits 2/3) — adjacency / external refs (partially decoded)

`nav_tclClusterInfo` describes a **neighbouring cluster**. In-memory layout (from the
constructor @0x00890fe0):

```
+0x00  rnw_tclClusterIDInternal   {u16 fileId, file offset, u16 size}
+0x0c  rnw_tclCoordInfo           (coordType + ref position)
+0x1c  rnw_tclOutline             (variable-length point list — cluster boundary)
+0x30  u16 status
```

On-disk element layout is **not fully decoded** (the variable-length outline makes the
elements non-fixed-size). Verified by data inspection of the EUR dataset:

- Each element embeds a **fileId** in the high 16 bits of a u32 (e.g. `55 4e`=20053,
  `fb 4e`=20219, `15 4f`=20245) with a low-16 offset/index — i.e. it names the
  neighbouring NAV file + position.
- Two trailing u32s in several elements decode as plausible PAU coordinates
  (`value * 180 / 2^31` degrees), consistent with a neighbour's reference position.
- Elements are variable-length (the outline), so `count` elements do **not** occupy a
  fixed `count × N` span; the reader must consume them structurally.

This is what resolves **external DirectCellRef refs (bit 15)**: a zerocell in cluster A
whose road continues into a neighbour refers to that neighbour's onecell index, and the
ClusterInfo list identifies which file/cluster that is. Consequence of leaving it
undecoded: rel8 roads whose FROM node lives in the neighbouring cluster are emitted as
single-point (the local TO node only) and are excluded from the distance-based join
index (`pts.len() >= 2`). ~571k of the 22.3M extracted EUR roads are single-point.

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

- onecell index = `(v & 0x3FF) - 1` — **1-based**, 10 bits
- bit 14 set → this zerocell is the road's **FROM** (start) node,
  else the **TO** (end) node (`bIsDirectionFrom` @0x008921e8 = `(v^0x8000)>>15`)
- bit 15 set → external reference (target onecell in a neighbouring cluster;
  resolved via ClusterInfo — not needed for local geometry)

`vAddCobZCToOC` @0x00890108 writes these into onecells:
FROM → onecell+0x28, TO → onecell+0x2A. Local clusters contain mostly TO refs;
FROM refs usually arrive from the neighbouring cluster's DCR list.

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

`hdr` bits (verified): 0–2 roadClass, 3 gateway, 4–6 networkClass,
13 link, 15 secundary. Observed (rc,nc) pairs concentrate on
(5,7) 792k, (6,7) 837k, (3,3) 107k, (2,2) 49k — the rest are rare.

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

Results on N6E2 L2 (70,504 MAP road ways):
- 1,830,749 RNW roads extracted from 8,257 files; 703,145 named (38%).
- **6,844 previously-unnamed MAP roads gained names** (all matched roads also
  get RNW class attributes `rn_class/rn_netclass/rn_link/rn_sec`).
- Named-road cross-check: 12,031/13,795 (87%) component names agree with the
  existing MAP name. Disagreements are concentrated in Budapest
  embankments/bridges where parallel named sections lie <30 m apart
  (genuinely ambiguous), plus sparse-RNW areas (HU/UA).
- Join method: a RNW road is a *component* of a MAP road when both its
  endpoints lie ≤30 m on the MAP polyline and ≥80 % of its points are
  ≤30 m from it; names/attributes are combined over all components.

Output format notes:
- JSONL: one road per line, keys `f,k,name,alt,rc,nc,link,sec,pts`; non-ASCII
  names as raw UTF-8, floats in native Rust display; `pts` = `[lon, lat]`
  degree pairs or null (no geometry).
- OSM output uses the canonical map2osm layout: one line per `<node>`; a
  `<way>` keeps all its `<nd>` on one line and all its `<tag>` on one line
  before `</way>`. Verified on N6E2 L2 (3,762,671 elements) via
  `osmium cat → PBF`.

Caveats:
- FROM nodes are only known locally when a DCR ref with bit 14 is present in
  the same cluster (rare); the extractor therefore usually emits
  `shapePts + [toNode]`. The join is distance-based, so the missing start
  node does not affect matching.
- External refs (bit 15) / ClusterInfo adjacency not fully resolved (§3a) — only
  needed for cross-cluster FROM-node assignment.
- Roads whose only local point is the TO node are emitted single-point and are
  excluded from the join index (`pts.len() >= 2`). On the full EUR dataset this is
  ~571k of 22.3M roads (POL alone: ~681k extracted, most multi-point).
- Cluster IDs (A) are not unique per file and sequence numbers (C) are not
  monotonic within a file — neither is safe as a key/ordering signal.

## 10. Dead ends (for the record)

- `N6E210I.TCI` cluster-list sections are zero-filled on disk — TCI is not a
  usable RNW↔MAP join path.
- The "UN" magic seen in NAV file headers is just fileId 20053 as u16 LE.
- MAP annotation layout ({u8 size,u8 type}) differs from RNW annotations
  ({u16 size,u16 type}) — mixing them up silently yields zero names.
