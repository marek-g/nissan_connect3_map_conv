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
| 0x00 | u16 | A | cluster ID (unique within dataset) |
| 0x02 | u16 | flags | bit 0x40 → coordType 4 (rel24), else 3 (rel16) |
| 0x04 | u32 | C | global sequence number (~unique, increasing) |
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
rnw_extract_rs [CCP_DIR] [OUT.jsonl]        # all N6E2 region folders; ~45 s for 8,257 files (I/O bound)
rnw_join_rs    RNW.jsonl MAP_L2.osm OUT.osm # OSM XML in and out; ~10 s for N6E2 L2
```

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
- External refs (bit 15) / ClusterInfo adjacency not resolved — only needed
  for cross-cluster FROM-node assignment.
- Roads with a single shape point and no local TO node have no geometry
  (267k of 682k in POL; excluded from the join index).

## 10. Dead ends (for the record)

- `N6E210I.TCI` cluster-list sections are zero-filled on disk — TCI is not a
  usable RNW↔MAP join path.
- The "UN" magic seen in NAV file headers is just fileId 20053 as u16 LE.
- MAP annotation layout ({u8 size,u8 type}) differs from RNW annotations
  ({u16 size,u16 type}) — mixing them up silently yields zero names.
