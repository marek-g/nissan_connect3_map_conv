# ROADMAP.md

## Project Vision & Goals

`nissan_connect3_map_conv` is an experimental research project focused on reverse-engineering the proprietary map data formats (`.MAP` and `.IDX`) used in legacy Nissan Connect (LCN2/LCN2kai) navigation systems.

The primary goal is to achieve interoperability by understanding how map data is structured, parsed, and ultimately rendered, bridging proprietary vehicle maps with open geographic standards.

---

## Architectural Decisions (ADR)

### 1. Choice of Intermediate Format: `.osm`

* **Decision:** We use the OpenStreetMap XML Format (`.osm`) as the primary intermediate data format for conversion and testing.
* **Rationale:**
* Standard open-source geographic format capable of storing complex topologies (nodes, ways, relations, and routing attributes).
* Highly compatible with the open-source GIS ecosystem (JOSM, QGIS, OsmAnd map creators).
* Simplifies data mapping when translating custom binary structures back and forth, avoiding custom ad-hoc schemas.

---

## Current Status (Proof of Concept)

* **Geometry Parsing:** Successfully decoding binary map structures to extract raw coordinates and generate vector geometry (streets, country borders) into GeoJSON for visualization and validation.
* **Research Phase:** Early exploratory stage. The project is experimental, non-production-ready, and serves as an independent technical exploration.

---

## Roadmap / Next Steps

### Phase 2: Attribute Extraction (In Progress)

* Map binary bitfields and property tables to extract street names, speed limits, and one-way flags.
* Validate attribute mapping against known geographic locations.

### Phase 3: Topology & Routing Analysis

* Investigate node connectivity, turn restrictions, and routing segments within `.MAP` / `.IDX` files.
* Test routing data structures using open-source tools.

### Phase 4: Bi-directional Conversion (Exploratory)

* Research the feasibility of converting updated `.osm.pbf` data back into the proprietary Bosch/Nissan format to test on actual hardware.

---

## Useful Resources & References

* **OpenStreetMap PBF Format Specification:**
Learn more about the binary structure of OpenStreetMap data on the [OSM Protocolbuffer Binary Format Wiki](https://wiki.openstreetmap.org/wiki/PBF_Format).
* **Download Map Extracts:**
Obtain regional `.osm.pbf` map files for testing and conversion from [Geofabrik Download Server](https://download.geofabrik.de/).
