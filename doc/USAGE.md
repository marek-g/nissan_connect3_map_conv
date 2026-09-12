# Install required tools

- `osmconvert` for conversion between `OSM XML` and `OSM PBF` formats
- `osmium` for splitting files

```shell
sudo apt install osmctools osmium-tool
```

# Build the converter

```bash
for p in map2osm osm2map rnw_extract rnw_join rnw2osm cprnav_compress cprnav_decompress; do (cd src/$p && cargo build --release); done
```

Binaries land in the shared cargo target dir (`.../release/map2osm`, `.../release/osm2map`).

# OSM → TravelMap conversion

## Download map in `osm.pbf` format

Go to http://download.geofabrik.de/ and dowanload Europe map ([Europe](http://download.geofabrik.de/europe-latest.osm.pbf) - 30+ GB) or a smaller region like [Małopolskie](http://download.geofabrik.de/europe/poland/malopolskie-latest.osm.pbf):

```shell
OSM2MAP=/home/marek/Ext/.cargo_cache/release/osm2map
MAP2OSM=/home/marek/Ext/.cargo_cache/release/map2osm

# create working folder
mkdir map
cd map

# download map
wget http://download.geofabrik.de/europe-latest.osm.pbf

# split map to smaller parts
#$BIN --emit-config=split.json
#osmium extract -c split.json europe-latest.osm.pbf
osmium extract -b 0.0000000,47.2499998,17.9999999,56.6999997 -o N6E1.osm.pbf europe-latest.osm.pbf
osmium extract -b 17.9999999,47.2499998,35.9999999,56.6999997 -o N6E2.osm.pbf europe-latest.osm.pbf

# convert each region
OSM2MAP_LOD=1 $OSM2MAP N6E2.osm.pbf out/MAP --region=N6E2

# convert back to verify
$MAP2OSM out/MAP -r N6E2 -l 123 -f pbf -o back
```

## Convert from `osm.pbf` to `osm` format

``` shell
osmconvert ./malopolskie-260824.osm.pbf -o=malopolskie-260824.osm
```



# OSM → TravelMap conversion

`osm2map` writes the same `.IDX` / `.MAP` / `.TCI` layout back from OSM data. The
input container — **OSM XML** or **OSM PBF** — is autodetected from the file extension
and first byte; both feed one identical classification pipeline, so the emitted binary
is byte-identical for the two encodings of the same map.

```bash
osm2map <in.osm|in.osm.pbf> <OUT_DIR> [W,S,E,N] [--region=NAME] [--bbox=W,S,E,N] [--pbf|--osm]

# Whole region from a PBF extract, writing region N6E2:
osm2map krakow.osm.pbf /tmp/out --region=N6E2

# Only a sub-box of a large XML file (positional bbox = legacy form):
osm2map malopolskie.osm /tmp/out 19.6,49.95,20.15,50.30 --region=N6E2
```

- `<in.osm|in.osm.pbf>` — input; `.pbf` extension wins, otherwise sniffed (`<` → XML, else PBF).
  Omit for `--region=N6E2` default input; use `--pbf` / `--osm` (`--xml`) to force a container.
- `<OUT_DIR>` — where `<REGION>AA.IDX` + `N6E2102.*` (land) + `N6E210I.*` (hydro) are written.
- `[W,S,E,N]` / `--bbox` — clip the input to a box (degrees); omit = the region's stock bounds.
- `--region=NAME` (or `OSM2MAP_REGION`) — target region code whose stock `AA.IDX` header + bounds
  seed the output (resinf catalog); must be a real stock region so W()/S()/E()/N() resolve.

Output is verified consistent both ways: full `malopolskie` via `.osm` and via `.osm.pbf`
produces byte-identical `IDX`/`MAP`/`TCI` (≈145 MB land MAP), and either output re-decodes
through `map2osm` (`-f xml` vs `-f pbf`) with zero element/tag differences.

## Region inventory (`--list-regions` and friends)

`osm2map` also enumerates the stock set (`STOCK_DIR`, default the unpacked `…/DATA/DATA/MAP`)
straight from the `*AA.IDX` headers — no OSM input needed. Each region's declared **profiles** are
read from its `AA.IDX` L0 tile slot (single, or a `multi` slot's sub-entries = the shard set that
region's signed resinf/RPI catalog declares).

```bash
osm2map --list-regions                       # table: name, bbox, dLon/dLat, #maps, MB, profiles
osm2map --region-of 19.9,50.1                # which region(s) contain a lon,lat (+ their profiles)
osm2map --emit-tsv    regions.tsv            # machine-readable per-region row (all 411)
osm2map --emit-config regions.json           # osmium `extract --config` JSON: per-region split
osm2map --emit-poly   regions.poly           # one multipolygon (single combined extract)
osm2map --emit-geojson regions.geojson       # GeoJSON rectangles (viewer overlay + labels)
osm2map --emit-profiles land_profiles.tsv    # region -> chosen land profile map (regenerate)
osm2map --list-regions --stock-dir /path/to/MAP   # override the stock dir
```

- `--region-of LON,LAT` finds the target region code from a coordinate (no hand-deriving it from the
  grid).
- `--emit-config` writes an **osmium extract config** (JSON) with one job per *covered* region (a
  region is "covered" if it ships a shard beyond the universal `0x12` hydro stub — 22 regions in this
  stock set). It drives a **single-pass** split of a big extract into per-region inputs:
  ```bash
  osm2map --emit-config regions.json
  osmium extract -c regions.json europe.osm.pbf     # -> ./N6E1.osm.pbf, ./N6E2.osm.pbf, ...
  ```
  (`regions.json` may also carry a top-level `"directory"`; see `osmium help extract` → CONFIG FILE.)
- `--emit-poly` writes every region as a rectangle polygon; use `osmium extract --polygon
  regions.poly -o all.osm.pbf` for one *combined* extract (a single region set, not a per-region
  split — for the split use `--emit-config`).
- `--emit-geojson` writes a GeoJSON `FeatureCollection`: one lon/lat rectangle per region (all 411),
  each with `name`/`label`/`text` = the `<REGION>` file prefix plus `covered`, `maps`, `map_bytes`,
  `profiles`, the bbox and SimpleStyle colours (covered vs off-coverage stub). Import it into any
  viewer (geojson.io, QGIS, MapLibre) to see which MAP region covers which part of the world. GeoJSON
  itself carries only the label *text* — font size/position are set by the viewer (it labels polygons
  at the centroid from `name`). A ready-made overlay with **big centred labels** ships at
  `gen/regions_map.html` (self-contained MapLibre page, opens by double-click, covered regions drawn
  bold + labelled; click a region for its `N<..>1XX.MAP` shards).
- Per-region `MAP` shard sizes and the profile → size map are shown so you can size memory and pick
  the emit profile before running a conversion.


### Profiles differ per region — why

A region's data is split into **profile shards** (`regProf` low byte), each a `<REGION>1XX.MAP`
file, and each region carries a *different set* of them: the shipped stock here has 9 shards for
`N6E1`, 3 for `N7E2`, and only the universal hydro `0x12` (`10I`) for the ~387 off-coverage stub
regions. The `0x12` hydro slot is the one id present in **all** 411 regions; every other id is
region-specific, and the *same* logical layer can carry a *different numeric id* per region (the
dominant base shard is `0x01` in `N6E1`, `0x2A` in `N6E2`, `0x16` in `N5E2`). That is because the
profile ids live in the region's **signed resinf/RPI metadata catalog**, which Bosch authors
per-region — there is no single global "layer ⇒ id" registry. Consequence for the writer: the
emitted land profile must be an id the target region already declares, or the head unit finds no
shard for it. The default `0x02` (car-validated on `N6E1`/`N6E2`) is therefore *not* universal.

`osm2map` now resolves this automatically per region (no env needed for the covered set). In
`setup_region` the emitted land profile is chosen by precedence:

1. `OSM2MAP_PROFILE=<id>` — manual override, always wins (one-off / experimentation).
2. the **curated land-profile map** (`--profiles <path>` / `OSM2MAP_PROFILES=<path>`, else the map
   embedded from `templates/land_profiles.tsv`) — the 22 covered regions, `0x02` where the region
   declares it, otherwise its largest declared shard. `--emit-profiles` regenerates this map.
3. fallback auto-pick for a region absent from the map: `0x02` if declared, else largest declared
   shard (`--list-regions` shows the sizes).

Whatever is chosen is then checked against the region's declared set; if it isn't declared the run
aborts with the region's profile list. Off-coverage stubs (only `0x12`) have no land shard to emit
into, so they correctly fail here.

### Should the converter emit multiple profiles? (analysis)

**Why a region ships several shards.** Decoding the stock set (per-shard *feature-kind* histograms,
via `map2osm`) shows a profile is a **container / size-and-entitlement shard of the region's
tiles — not a semantic layer**. In `N6E2` the roads, POI, areas and water all re-appear across the
`0x02`, `0x2A`, `0x11`, `0x0E` shards; individual tiles land in whichever shard has room / matches
the delivery, and the *kind* of a cell comes from the per-cell feature code at the cell header, not
from the profile id (`MAP_format` §11: the profile field at `MAP+0x1e` is not read by the runtime
header ctor). Ghidra confirms the read-side model in `DAPIAPP.OUT` (ARM:LE:32):

- `dap_map_tclRegProfList` = a `vector<RegProfListDesc>` (each record 4×u16, field0 = `regProf`,
  built by `bStoreRegProfListDesc`) plus a parallel `vector<RegProfListIdxFileIDList>`; the id
  controller creates a shard on demand with `bCreateRegionProfileWithIdxId` (`0x008db808`) /
  `bAddIdxId2RegionProfile` (`0x008da664`) and indexes it by `regProf`.
- tile lookup is **offset based**: `dap_map_tclMapFileOffset` (2×u32 = file,offset). A tile carries
  its own regProf + offset, so the loader jumps straight to a tile in the right shard — it never
  needs the whole region, and it does not care which regProf a shard *was*, only that an entry with
  the tile's regProf exists in the RPI.

So the region's shard split is a *packaging* decision (Bosch authors the RPI catalog per region),
and content is **not** bound to a particular id.

**What the converter must guarantee.** Only that the regProf it stamps on its tiles is an id the
target region's signed RPI/`AA.IDX` **declares** — i.e. the id-controller can resolve it. Any single
declared id renders identically, because each emitted cell carries its own feature code. That is why
the whole `N6E2` region converts correctly into the single `0x02` shard and byte-matches stock on the
`AA.IDX`/`TCI`.

**Recommendation.** Keep emitting **one land shard per region** (the auto-picked declared id). It is
functionally complete and validated. Multi-shard emission would only add value for very large
regions where the device's per-shard paging or an exact RPI replication matters — and doing it
faithfully would require reproducing Bosch's per-tile→shard offset assignment byte-for-byte (the RPI
is signed, so the converter cannot author new profile ids anyway). For re-authoring existing
coverage, one declared shard per region is the correct and sufficient target.


# TravalMap -> OSM conversion

## MAP → OSM (XML or PBF)

Please note that current converter assumes that all files are uncompressed under the same name. The decompressor can be found here: https://github.com/sapphire-bt/lcn2kai-decompress

```bash
map2osm <IDX_file | MAP_dir> [-r REGIONS] [-l LEVELS] [-b W,S,E,N|none] [-f xml|pbf] [-o OUT_DIR]

# Poland, all detail levels, OSM XML:
map2osm .../DATA/DATA/MAP -r N6E1,N6E2 -l 123 -o /tmp/pl

# Same, but emit compact OSM PBF instead of XML:
map2osm .../DATA/DATA/MAP -r N6E1,N6E2 -l 123 -f pbf -o /tmp/pl
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
rnw_extract <CCP_dir> RNW.jsonl [-b W,S,E,N|none]   # ~70 s for all 8,257 files (whole EUR)
rnw_join    RNW.jsonl /tmp/pl/N6E2_L2.osm /tmp/pl/N6E2_L2_rnw.osm   # ~10 s
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
rnw2osm <NAV*.DAT | dir>... [-o OUT.osm] [--outlines] [-b W,S,E,N|none] [-s METERS] [--no-stitch] [--no-snap] [--secondary] [--level N]

# Poland road network as one OSM file (~17 s, ~1.93M roads):
rnw2osm .../DATA/DATA/RNW/CCP/POL -o /tmp/pl_roads.osm

# Krzeszowice (near Kraków) with cluster outlines, ready for JOSM:
rnw2osm .../DATA/DATA/RNW/CCP/POL -b 19.50,50.05,19.88,50.28 --outlines -o /tmp/krzeszowice.osm
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
