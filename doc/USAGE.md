# TravalMap -> OSM conversion

## Build

```bash
for p in map2osm_rs osm2map_rs rnw_extract_rs rnw_join_rs rnw2osm_rs; do (cd src/$p && cargo build --release); done
```

Binaries land in the shared cargo target dir (`.../release/map2osm_rs`, `.../release/osm2map_rs`).

## MAP → OSM (XML or PBF)

Please note that current converter assumes that all files are uncompressed under the same name. The decompressor can be found here: https://github.com/sapphire-bt/lcn2kai-decompress

```bash
map2osm_rs <IDX_file | MAP_dir> [-r REGIONS] [-l LEVELS] [-b W,S,E,N|none] [-f xml|pbf] [-o OUT_DIR]

# Poland, all detail levels, OSM XML:
map2osm_rs .../DATA/DATA/MAP -r N6E1,N6E2 -l 123 -o /tmp/pl

# Same, but emit compact OSM PBF instead of XML:
map2osm_rs .../DATA/DATA/MAP -r N6E1,N6E2 -l 123 -f pbf -o /tmp/pl
```

- `-r` — exact region codes, comma-separated (`N6E1` ≠ `N6E10`); omit = all 411 regions
- `-l` — levels: L0 = whole-region outline, L1–L3 increasing detail (default `123`)
- `-b W,S,E,N` — keep only tiles whose extent overlaps the box (degrees); a tile-level selection, **not** a geometry clip. Default `none` = whole region.
- `-f xml|pbf` — output container (default `xml`). `pbf` requires `-o`; writes compact `.osm.pbf` (dense-node) files that open directly in JOSM/osmium and are ~8–15× smaller than the XML.
- Output: `OUT_DIR/<REGION>_L<level>.osm` (or `.osm.pbf`) — POIs as `<node>`, lines as open `<way>`, polygons as closed `<way>`. Tags: `name`, `name:alt`, `ref`, plus original properties under `tm:*` and decoded annotation payloads (`tm:surface`, `tm:elev`, `tm:water_class/type`, `tm:netclass`, `tm:xfree`, `tm:roadinfo`, `tm:city_display/size/admin/overlap` — see `TravelMap_format/02 - details/MAP_format.md` §8).
- Both containers carry the **same** objects, ids, versions, timestamps and tags — only the encoding differs. Coordinates agree to the PBF resolution grid (integer nanodegrees, ~1e-7° ≈ 10 cm); XML is written at `{:.8}`°.
- N6E2 L2 ≈ 560 MB in ~6 s. A full-world conversion is multi-GB — convert per region and/or gzip; `-f pbf` is the compact alternative.

## 3. Road names from RNW (optional)

```bash
rnw_extract_rs <CCP_dir> RNW.jsonl [-b W,S,E,N|none]   # ~70 s for all 8,257 files (whole EUR)
rnw_join_rs    RNW.jsonl /tmp/pl/N6E2_L2.osm /tmp/pl/N6E2_L2_rnw.osm   # ~10 s
```

Adds `name`/`name:alt` (to unnamed roads) and, to every matched road way
(`tm:layer="road"`), the RNW class attributes `rn_class/rn_netclass/rn_roadtype/
rn_link/rn_sec/rn_freeway` plus a derived OSM `highway=*` tag (motorway…unclassified,
from the runtime's display-class table — see `TravelMap_format/02 - details/RNW_format.md` §6a).
All other elements pass through unchanged.

- `-b W,S,E,N` — geographic sanity filter (degrees) for the cluster scan. Default
  `-30,30,60,75` covers the whole EUR dataset (Iceland..Turkey). Use a tighter box to
  speed up a single-area conversion; `none` disables it (diagnostics only — see
  `TravelMap_format/02 - details/RNW_format.md` §9 for why the filter matters).

## 3b. RNW → OSM XML (standalone, for visualization)

```bash
rnw2osm_rs <NAV*.DAT | dir>... [-o OUT.osm] [--outlines] [-b W,S,E,N|none] [-s METERS] [--no-stitch] [--no-snap] [--secondary] [--level N]

# Poland road network as one OSM file (~17 s, ~1.93M roads):
rnw2osm_rs .../DATA/DATA/RNW/CCP/POL -o /tmp/pl_roads.osm

# Krzeszowice (near Kraków) with cluster outlines, ready for JOSM:
rnw2osm_rs .../DATA/DATA/RNW/CCP/POL -b 19.50,50.05,19.88,50.28 --outlines -o /tmp/krzeszowice.osm
```

> **`-b` selects clusters *geometrically*:** a cluster is included when its outline footprint
> (the boundary polygon the map stores for it) overlaps the box — falling back to its origin
> point if it has no outline. A cluster is a large tile whose roads spill well past its origin,
> so a box that hugs the target still pulls in every tile that touches it; no padding needed.
> (An always-on sanity check still rejects clusters whose origin falls outside the dataset's
> plausible bounds, which catches a handful of garbage decodes.) It is a selection filter only,
> not a clip — emitted ways are not cut to the box.

Decodes the RNW clusters directly into OSM (roads as open `<way>`, junctions as
`<node>`), tagging each way with `highway=*` (from the runtime display-class table),
`name`, OSM-standard attributes derived from the onecell header (`tunnel=yes`,
`bridge=yes`, `junction=roundabout`, and `oneway` = `yes`/`-1` for forward/reverse travel
along the stored geometry), the raw RNW class fields (`rn_class`, `rn_netclass`,
`rn_roadtype`, `rn_link`, `rn_sec`, `rn_freeway`) plus the remaining header flags and the
stored `rn_length` (all emitted only when set / non-zero — absence means "not flagged"),
and provenance (`rnw_file`, `rnw_cluster`, `rnw_oncell_index`). `rnw_oncell_index` is the
onecell's index within its cluster — together with the file + cluster it is the road's
unique source identity (the format stores no separate global road id; every up/down/overlap
reference points to a road by this index).
Node IDs are negative, way IDs positive. This is the tool for eyeballing the decoded
road network; it is independent of the `.MAP`-based pipeline above.

Every **primary** onecell is emitted: shaped ones as `[fromNode] + shape + [toNode]`, and
the majority that carry **no inline shape** as a straight `[fromNode, toNode]` segment
(see `RNW_format.md` §6). Skipping the straight ones drops ~60% of the network. A road can
also have a coarser **secondary** LOD copy (`bIsSecundary`, header bit 15) stored in a
neighbouring cluster with the same geometry; the app always renders the primary, so by
default the secondaries are dropped (each road once, at full detail). `--secondary` emits that
coarser layer on its own instead.

Each cluster stores its own copy of a shared boundary junction and the copies differ by a
few PAU (~0.06–0.08 m), so exact-coordinate dedup severs every road at a cluster edge.
Boundary nodes are unified in the runtime's order — **overlap links → border marker →
proximity** (`RNW_format.md` §3c):

- `-s METERS` — snap radius for the marker + proximity merges (default `1.0`; `0` = exact
  match only, i.e. overlap-links-only). A junction whose zerocell carries the RNW **border
  marker** (rim / cpx-crossing flag or a `0x31` annotation — the runtime's own
  `bBordersObjectAtTo/From` test) merges with its nearest *marked* twin; an unmarked
  junction merges with any nearby twin. That proximity step is what keeps the network
  connected, because the markers cover only ~10% of nodes. On full Poland the default still
  yields ~3.72M nodes / one large connected component.
- `--no-stitch` — disable **cross-cluster overlap stitching** (on by default): each onecell's
  Overlaps list (`RNW_format.md` §3b) is followed to the named neighbour and the shared node
  merged, so a road continues across the edge exactly as the app renders it.
- `--no-snap` — disable the proximity fallback for **unmarked** junctions; only marked nodes
  and overlap links are then merged. This is the purest faithful mode (the runtime itself
  never merges by distance), but the network fragments to ~31% in one component on Kraków —
  most boundary junctions carry no marker. See `RNW_format.md` §3c for the measured split.
- `--secondary` — emit **only** the **secondary** (`bIsSecundary`) LOD layer instead of the
  primaries (default). This isolates the coarser cross-cluster duplicate of each road — the
  simplified copy the app keeps for low zoom (a road stored twice: once detailed/primary, once
  coarser/secondary). On Krzeszowice the box holds 9,649 primary vs 3,675 secondary onecells;
  the Balice I interchange is the clearest example (primary in one cluster, a coincident 2-pt
  secondary copy in its neighbour). Overlap stitching resolves a link that points at a layer we
   did not emit to that twin's emitted copy (`oGetPrimaryOverlap`), so both modes stay stitched.
- `--level N` — which **cluster tier** to include (a second, independent detail axis from
   primary/secondary; `RNW_format.md` §2, overview §4.13). The clusters come in two interleaved
   tiers: a **coarse** layer (main roads) and a **fine** layer (`flags` word `u16@2 == 0`) that
   carries the dense residential grid. `--level 0` emits only the coarse tier; `--level 1`
   (the default) includes the fine tier too. This dataset has no level beyond 1 — a coarse road
   refines into the fine tier via down-cells exactly one step deep. **Use the default (`1`) for a
   complete street map:** with `0`, housing-estate streets vanish (motorways and main roads remain).

   At `--level 1` the two tiers are ~90% complementary, but a road can exist in both. To avoid
   drawing such a road twice, a coarse onecell is **dropped when it is fully refined into fine
   sub-segments that are present in this run** (the data's own down-cell links decide this — it is
   not a geometry guess). This never drops a road that has no fine counterpart and never loses a
   name; the summary line reports how many were dropped as `refined_dropped=N` (e.g. 1010 for the
   Kraków box).
- `--countbox W,S,E,N` — diagnostic: count the roads (grouped by `highway` class) that fall inside
   the box across all parsed clusters, and how many of their clusters would be outline-selected,
   then exit. Handy for checking how much data a region actually holds before converting it.

## 4. Verify / load

```bash
osmium cat file.osm -f pbf -o file.pbf        # validity check
```

Then open the `.osm` (or the PBF) in JOSM.

# Cut off the smaller part of the OSM map

``` shell
sudo apt install osmium-tool
osmium extract -b min_lon,min_lat,max_lon,max_lat duzy_plik.osm.pbf -o maly_wycinek.osm.pbf

osmium extract -b 19.78,49.95,20.21,50.15 malopolskie-260824.osm.pbf -o krakow.osm
osmium extract -b 19.91,50.03,20.03,49.99 malopolskie-260824.osm.pbf -o krakow_pd.osm

osmium extract -b 19.58,50.12,19.67,50.17 malopolskie-260824.osm.pbf -o krzeszowice.osm
```

# Open `osm` format with `JOSM` (Java OpenStreetMap Editor)

Download `josm-tested.jar` from https://josm.openstreetmap.de/wiki/Download

``` shell
java -jar josm-tested.jar
```

It creates `~/.config/JOSM` folder with its settings.

# Download OSM map

## Download map in `osm.pbf` format

Go to http://download.geofabrik.de/ and choose a map. For example: http://download.geofabrik.de/europe/poland/malopolskie-latest.osm.pbf

## Convert from `osm.pbf` to `osm` format

``` shell
sudo apt install osmctools

osmconvert ./malopolskie-260824.osm.pbf -o=malopolskie-260824.osm
```

# OSM → TravelMap conversion

`osm2map_rs` writes the same `.IDX` / `.MAP` / `.TCI` layout back from OSM data. The
input container — **OSM XML** or **OSM PBF** — is autodetected from the file extension
and first byte; both feed one identical classification pipeline, so the emitted binary
is byte-identical for the two encodings of the same map.

```bash
osm2map_rs <in.osm|in.osm.pbf> <OUT_DIR> [W,S,E,N] [--region=NAME] [--bbox=W,S,E,N] [--pbf|--osm]

# Whole region from a PBF extract, writing region N6E2:
osm2map_rs krakow.osm.pbf /tmp/out --region=N6E2

# Only a sub-box of a large XML file (positional bbox = legacy form):
osm2map_rs malopolskie.osm /tmp/out 19.6,49.95,20.15,50.30 --region=N6E2
```

- `<in.osm|in.osm.pbf>` — input; `.pbf` extension wins, otherwise sniffed (`<` → XML, else PBF).
  Omit for `--region=N6E2` default input; use `--pbf` / `--osm` (`--xml`) to force a container.
- `<OUT_DIR>` — where `<REGION>AA.IDX` + `N6E2102.*` (land) + `N6E210I.*` (hydro) are written.
- `[W,S,E,N]` / `--bbox` — clip the input to a box (degrees); omit = the region's stock bounds.
- `--region=NAME` (or `OSM2MAP_REGION`) — target region code whose stock `AA.IDX` header + bounds
  seed the output (resinf catalog); must be a real stock region so W()/S()/E()/N() resolve.

Output is verified consistent both ways: full `malopolskie` via `.osm` and via `.osm.pbf`
produces byte-identical `IDX`/`MAP`/`TCI` (≈145 MB land MAP), and either output re-decodes
through `map2osm_rs` (`-f xml` vs `-f pbf`) with zero element/tag differences.

