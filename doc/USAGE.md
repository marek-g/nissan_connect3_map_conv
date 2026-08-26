# TravalMap -> OSM conversion

## Build

```bash
for p in map2osm_rs rnw_extract_rs rnw_join_rs; do (cd $p && cargo build --release); done
```

## MAP → OSM XML

Please note that current converter assumes that all files are uncompressed under the same name. The decompressor can be found here: https://github.com/sapphire-bt/lcn2kai-decompress

```bash
map2osm_rs <IDX_file | MAP_dir> [-r REGIONS] [-l LEVELS] [-o OUT_DIR]

# Poland, all detail levels:
map2osm_rs .../DATA/DATA/MAP -r N6E1,N6E2 -l 123 -o /tmp/pl
```

- `-r` — exact region codes, comma-separated (`N6E1` ≠ `N6E10`); omit = all 411 regions
- `-l` — levels: L0 = whole-region outline, L1–L3 increasing detail (default `123`)
- Output: `OUT_DIR/<REGION>_L<level>.osm` — POIs as `<node>`, lines as open `<way>`, polygons as closed `<way>`. Tags: `name`, `name:alt`, `ref`, plus original properties under `tm:*`.
- N6E2 L2 ≈ 560 MB in ~6 s. A full-world conversion is multi-GB — convert per region and/or gzip.

## 3. Road names from RNW (optional)

```bash
rnw_extract_rs <CCP_dir> RNW.jsonl            # ~45 s for all 8,257 files
rnw_join_rs    RNW.jsonl /tmp/pl/N6E2_L2.osm /tmp/pl/N6E2_L2_rnw.osm   # ~10 s
```

Adds `name`/`name:alt` and `rn_class/rn_netclass/rn_link/rn_sec` to road ways (`tm:layer="road"`); all other elements pass through unchanged. Note: the extractor's cluster filter is currently tuned to the N6E2 area (~12–37°E, 46.5–57.5°N).

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

# OSM -> TravelMap conversion

Not implemented yet

