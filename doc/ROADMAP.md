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

CPRNAV_2-compressed files (all LID; some MAP/IDX) are unpacked first by `cprnav_decompress_rs` so the
decoders read plain bytes. Both per-block header widths are handled — 16-bit vs 32-bit, selected by
`block_size = unknown × 0x400` (the difference the reference tool got wrong on LID).

---

## Current Status

The **read direction** (TravelMap → OSM) works for all three layers; the **write direction** is the
open frontier.

| Format | Decode | Extract / convert to OSM | Write (generate) |
|--------|:------:|--------------------------|:----------------:|
| **MAP / IDX** | ✅ | ✅ `map2osm_rs` → OSM XML (POIs, lines, polygons + decoded annotations) | ⚠️ guide written; not yet implemented |
| **RNW** | ✅ (incl. AEX direction-of-travel) | ✅ `rnw_extract_rs` + `rnw_join_rs` → road names + class attributes onto OSM | ❌ |
| **LID** | ✅ structure (block header, POI records, text pool, categories); `GLOB_POI` = SQLite FTS3 | ⚠️ partial — point-POIs readable, no exporter yet | ❌ (byte-level gaps remain) |

Tooling (`src/`, Rust): `map2osm_rs`, `rnw_extract_rs`, `rnw_join_rs`, `cprnav_decompress_rs`.

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

### Phase 2: RNW writer

Generate `.RNW` clusters + the `NAV_ROOT.DAT` index from a road graph (OSM ways/nodes). Highest value
for producing a map that actually routes.

### Phase 3: MAP / IDX writer

Generate `.IDX` tile tables + `.MAP` blocks (marker, 3-list cells, point pool, annotations, text) from
OSM ways/nodes/relations. The [writer guide](TravelMap_format/03%20-%20writer%20guide/writer_guide.md)
already separates byte-perfect fields from bypass-able ones; this turns that into a working generator, with
`map2osm_rs` as the reference data model.

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
