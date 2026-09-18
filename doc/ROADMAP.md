# ROADMAP.md

## Project Vision & Goals

`nissan_connect3_map_conv` is a research project reverse-engineering the proprietary **Bosch
"TravelMap"** map formats used in legacy Nissan Connect (LCN2KAI) navigation systems. The goal is
full interoperability: understand how the car's map data is structured, parsed, and rendered — and
bridge it with open geographic standards, supporting both **extraction to OSM** (working for all
layers) and, eventually, **generation back into the proprietary format**.

The firmware exposes three data categories under `CRYPTNAV/DATA/DATA/`:

| Category | What it is | Docs |
|----------|-----------|------|
| **MAP / IDX** | The *drawing* layer — rendered base-map geometry (roads, water, areas, POI points) plus the tile index that locates it. | [overview](TravelMap_format/01%20-%20overview/01_MAP_overview.md) · [byte-level](TravelMap_format/02%20-%20details/MAP_format.md) |
| **RNW** | The *routing* layer — the road network as a graph (roads + intersections), what the engine walks to compute a route. | [overview](TravelMap_format/01%20-%20overview/02_RNW_overview.md) · [byte-level](TravelMap_format/02%20-%20details/RNW_format.md) |
| **LID** | The *content* layer — named POIs / landmarks, map objects, and text resources for search + rendering. | [byte-level](TravelMap_format/02%20-%20details/LID_format.md) |

---

## Documentation Map

Docs are organised in three tiers under `doc/`:

- **`TravelMap_format/01 - overview/`** — plain-language, non-programmer explanations (the "why").
  - [`01_MAP_overview.md`](TravelMap_format/01%20-%20overview/01_MAP_overview.md) — `.IDX`/`.MAP`
  - [`02_RNW_overview.md`](TravelMap_format/01%20-%20overview/02_RNW_overview.md) — `.RNW`
- **`TravelMap_format/02 - details/`** — precise byte-level specs (offsets, sizes, bit fields).
  - [`MAP_format.md`](TravelMap_format/02%20-%20details/MAP_format.md) · [`RNW_format.md`](TravelMap_format/02%20-%20details/RNW_format.md) · [`LID_format.md`](TravelMap_format/02%20-%20details/LID_format.md)
- **`TravelMap_format/03 - writer guide/`** — how to *generate* your own files.
  - [`writer_guide.md`](TravelMap_format/03%20-%20writer%20guide/writer_guide.md) — currently covers `.IDX`/`.MAP` only
- **[`USAGE.md`](USAGE.md)** — build + run the tools.

---

## Architectural Decisions (ADR)

### 1. Intermediate format: OSM XML

* **Decision:** We use the OpenStreetMap XML Format (`.osm`) as the primary intermediate data format for conversion and testing.
* **Rationale:**
  * Standard open-source geographic format capable of storing complex topologies (nodes, ways, relations, and routing attributes).
  * Highly compatible with the open-source GIS ecosystem (JOSM, QGIS, OsmAnd map creators).
  * Simplifies data mapping when translating custom binary structures back and forth, avoiding custom ad-hoc schemas.

### 2. Tooling: small zero-dependency Rust binaries

Each converter/extractor is a standalone Rust binary under `src/` (`cargo build --release`), with no
heavy GIS dependencies. OSM XML in/out keeps every artefact human-inspectable and diffable, which is
essential for validating a reverse-engineered format.

### 3. Decompression as a separate pre-step

CPRNAV_2-compressed files (all LID; some MAP/IDX) are unpacked first by `cprnav_decompress` so the
decoders read plain bytes. Both per-block header widths are handled — 16-bit vs 32-bit, selected by
`block_size = unknown × 0x400` (the difference the reference tool got wrong on LID).

---

## Current Status

The **read direction** (TravelMap → OSM) works for all three layers. The **write direction** is now
demonstrated for MAP/IDX (`osm2map`, byte-matches stock on `AA.IDX`) and, newly, **RNW**
(`osm2rnw`, round-trips through `rnw2osm` with faithful geometry / connectivity / names / classes).

| Format | Decode | Extract / convert to OSM | Write (generate) |
|--------|:------:|--------------------------|:----------------:|
| **MAP / IDX** | ✅ | ✅ `map2osm` → OSM XML (POIs, lines, polygons + decoded annotations) | ✅ `osm2map` → `.IDX`/`.MAP` (byte-matches stock; re-decodes clean) |
| **RNW** | ✅ (incl. AEX direction-of-travel) | ✅ `rnw2osm` → road graph as OSM (class → `highway`, names, oneway/tunnel/bridge/roundabout) | ✅ `osm2rnw` → `NAVnnnnn.DAT` clusters + `.tci` locator (`--tci`, offline-validated); ⚠️ on-device boot untested |
| **LID** | ✅ structure (block header, POI records, text pool, categories); `GLOB_POI` = SQLite FTS3 | ⚠️ partial — point-POIs readable, no exporter yet | ❌ (byte-level gaps remain) |

Tooling (`src/`, Rust): `map2osm`, `osm2map`, `rnw2osm`, `osm2rnw`, `cprnav_compress`, `cprnav_decompress`.
(`rnw_extract` / `rnw_join` are the older name-annotation path, superseded by the standalone `rnw2osm`.)

> For navigation alone the LID content layer is optional — the network loads and routes via
> RNW→MAP without it. LID matters for POI search / landmark rendering.

---

## Roadmap / Next Steps

The remaining work is the **write direction** — generating files the runtime (`DAPIAPP.OUT`) will load.
Ordered by value:

### Phase 1: Finish LID read → POI exporter

Close the record-level gaps listed in `LID_format.md` §9 (exact header field offsets, sequence-TOC
framing, text-pool resolution, line/polygon layouts), then emit an OSM/CSV POI export from unpacked LID
cross-referenced with `GLOB_POI`.

### Phase 2: RNW writer — `osm2rnw`

Generate `.RNW` clusters (`NAVnnnnn.DAT`) from a road graph (OSM ways/nodes). **Implemented and validated
for geometry:** a quadtree splits the network into clusters (≤ 1024 onecells each — the DCR/overlap ref
packs the onecell index in 10 bits), each way is broken at every vertex into straight onecells, and every
shared vertex is duplicated at *identical* PAU across its clusters with a **border (rim) marker** on each
copy. Round-trip through `rnw2osm` is faithful — geometry, connectivity, street names and
`highway=*` classes all preserved, cross-cluster junctions stitching via the marker even under
`--no-snap`. Design rationale + layout: `writer_guide.md` §7. Two items remain:

- **2a — explicit overlap links (ci2).** `osm2rnw` currently relies on the border-marker stitch (the
  runtime's documented fallback). Stock data *also* writes ci2 overlap records naming the neighbour
  onecell. Emitting those needs a second serialisation pass (owner writes the neighbour's
  file-offset/fileId/origin into its ci2 records once all cluster offsets are fixed). This makes output
  byte-faithful to stock on the cross-cluster link path; validate with `rnw2osm … overlaps=N>0`.
- **2b — the cluster locator (`.tci`).** Mechanism **resolved** by Ghidra and **implemented** in `osm2rnw
  --tci`. TCI reaches the reader through two carriers: per-tile `.tci` shards under `data/data/map/`
  (4-level tile index → `{u32 fileOffset, u16 length, u16 fileId}` → `NAV%05u.DAT`; ref word order
  re-confirmed 2026-09 from `u16LoadClusterIdListAndStoreInQ` disasm @0x8deb20 — `osm2rnw` had it swapped
  and is fixed) and the per-region `NAV_ROOT.DAT` (which also embeds a TCI). The runtime registers shards
  by directory scan of `data/data/map/*.tci`, taking tile ids from the FILENAMES (`u16InitTciFileList`
  @0x8de860 → `u16InitTciIdList` @0x8de624). Key facts: `fileId` is the literal `%05u` filename
  (`vFileId2Name`); the ref `fileOffset` packs the region ident in bits 0–13 (`u16GetRegionIdent`) and the
  16 KB-aligned cluster offset in bits 14+; the reader reads `nPrim` refs (write `nPrim==nAll`); routing
  queries the finest level only, so each cluster is registered in every finest-level tile its bbox
  overlaps. `osm2map` no longer emits `.tci`; `osm2rnw` reads the tile grid from step-1 `<REGION>AA.IDX`
  (`--map-idx`). The `regionIdent` table is **FINAL 2026-09**, all 17 regions (`REGION_IDENT` /
  `doc/region_ident.tsv`) from two mutually-confirming methods: shard ref→NAV join (DEU 0x401 IBE 0x409
  FRM 0x40a ISV 0x40b SCA 0x40d EEU 0x42a) and the ci-adjacency scan reproducing all six + the other 11
  (POL 0x402 GRC 0x403 TUR 0x404 BNL 0x407 ACL 0x408 GBI 0x40c CHS 0x40e ELL 0x411 INT 0x412 EAD 0x416
  MLC 0x483); the old NAV_ROOT-histogram values (FRM 0x403, GRC 0x406, TUR 0x407, BNL 0x404, MLC 0x40a)
  were wrong. **Two osm2rnw packing bugs fixed:** .tci ref word order (`off,len,fid`) and the ci2
  neighbour `fileOffset` now packs regionIdent like stock (caught by `rnwcheck --generated` ci scan).
  Default shard name fixed: `--tci-prof 0x02` → `N6E2102.TCI` matching the osm2map land shard, because
  the runtime parses tile ids from the FILENAME at dir-scan. **POL blocker RESOLVED (hypothesis) 2026-09-18:**
  Kraków's stock shard `N6E2102.TCI` is an empty stub (ref pool zero-filled) and no shard references POL's
  clusters, yet stock routing works in Kraków — the per-region `NAV_ROOT.DAT` is now DECODED (device writer
  `rnw_tclNavRootKnitter` recovered): it carries a **root-cluster list** of 24-byte `nav_tclClusterInfo`
  gateway records (POL: 1 cluster in `NAV00001` @0x4000 len 0xe660) from which ci-adjacency reaches every
  cluster — no TCI needed (all 17 stock roots' packed idents re-confirm the final region table).
  **Remaining:** `osm2rnw` must emit a minimal `NAV_ROOT.DAT` (root-cluster ListDesc + empty annots + the
  appended global-area/instruction records); on-device `routeprobe` capture still confirms the load order;
  Patches: cluster flags byte bit 0x80 triggers a `NAV____n.PTH` memcpy — keep it clear and
  drop stale `data/connect/rnw/**/*.PTH`.

### Phase 3: MAP / IDX writer

Generate `.IDX` tile tables + `.MAP` blocks (marker, 3-list cells, point pool, annotations, text) from
OSM ways/nodes/relations. The [writer guide](TravelMap_format/03%20-%20writer%20guide/writer_guide.md)
already separates byte-perfect fields from bypass-able ones; this turns that into a working generator, with
`map2osm` as the reference data model.

### Phase 4: LID writer + compressor

Generate LID content blocks (POI sequences + text pool) and re-compress with a CPRNAV_2 **encoder**
(only a decompressor exists today). Pin the numeric `CAT_ID` → category table.

### Phase 5: End-to-end OSM → TravelMap round-trip

Compose the MAP/IDX + RNW + LID writers into one pipeline and validate the output by loading it in
`DAPIAPP.OUT`.

---

## Useful Resources & References

* **OpenStreetMap PBF Format Specification:**
  Learn more about the binary structure of OpenStreetMap data on the [OSM Protocolbuffer Binary Format Wiki](https://wiki.openstreetmap.org/wiki/PBF_Format).
* **Download Map Extracts:**
  Obtain regional `.osm.pbf` map files for testing and conversion from [Geofabrik Download Server](https://download.geofabrik.de/).
