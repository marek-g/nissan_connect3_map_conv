#!/usr/bin/env bash
# Małopolska (Geofabrik malopolskie PBF) -> Bosch TravelMap region N6E2 deploy pack.
#
# Pipeline (same recipe as gen/pipeline.sh / validated on-car trials #21/#25):
#   osm2map (PBF -> decompressed N6E2AA.IDX + N6E2102/10I.MAP)
#     -> tmcheck.py structural gate
#     -> cprnav_compress --level 9 into the card layout:
#          DATA/DATA/MAP/     N6E2AA.IDX, N6E2102.MAP, N6E210I.MAP  (compressed)
#          DATA/CONNECT/MAP/  N6E2AA.IDX                            (compressed)
#     -> every packaged file is decompressed back and cmp'd against the raw source.
#
# Output lands next to this script (./DATA/...). Scratch (raw MAP/IDX,
# verify copies) goes to /tmp/opencode/ per the repo rule. Re-runnable: rm -rf's its own
# scratch and DATA/ on every run. Peak RAM is in the osm2map parse+tile phase (a full
# voivodeship PBF; #23-scale loads) — free memory there if it gets OOM-killed.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"

OSM="${OSM:-/home/marek/Ext/reverse_engineering/NissanMaps/OSM-map/malopolskie/malopolskie-260824.osm.pbf}"
REGION="${REGION:-N6E2}"                      # stock region containing the whole voivodeship
#STOCK_DIR="${STOCK_DIR:-/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/MAP}"

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/home/marek/Ext/.cargo_cache}"
O2M="${O2M:-$CARGO_TARGET_DIR/release/osm2map}"
CPC="${CPC:-$CARGO_TARGET_DIR/release/cprnav_compress}"
CPD="${CPD:-$CARGO_TARGET_DIR/release/cprnav_decompress}"

RAW="${RAW:-/tmp/opencode/malopolska_raw}"
VERIFY=/tmp/opencode/malopolska_verify
PKG="$HERE"                                      # deploy root: $PKG/DATA/{DATA,CONNECT}/MAP
LOGS="$HERE/logs"

# car-validated env (trials #21/#25 = the current defaults of osm2map).
# LOD=1 is REQUIRED here (not the sparse-extract default): whole-voivodeship density with
# LOD off floods the Kraków tiles past the 15-entry IDX multi-slot cap (measured: L1/5=108,
# L2/545=21 -> tmcheck FAIL; the head unit reads the slot count as a 4-bit field). LOD on
# uses the per-level floors MEASURED from the stock N6E2 dense tiles (Kraków/Warszawa);
# its former #23 crash cause was the bare feat-0x0001 name bug, fixed in the current source.
export OSM2MAP_LOD="${OSM2MAP_LOD:-1}"
export OSM2MAP_MODE="${OSM2MAP_MODE:-full}"
export OSM2MAP_REGION="$REGION"
#export STOCK_DIR

LAND_P="102"        # base32(0x02) land shard car-validated on N6E2; hydro land-mode stub is 10I
FILES=("${REGION}AA.IDX" "${REGION}${LAND_P}.MAP" "${REGION}10I.MAP")

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*"; }

# ---- 0. binaries (fresh from current source; SKIP_BUILD=1 to reuse) -----------------
if [ "${SKIP_BUILD:-0}" != 1 ]; then
  for c in osm2map cprnav_compress cprnav_decompress; do
    log "cargo build --release $c"
    CARGO_TARGET_DIR="$CARGO_TARGET_DIR" cargo build --release -q --manifest-path "$REPO/src/$c/Cargo.toml"
  done
fi
[ -x "$O2M" ] && [ -x "$CPC" ] && [ -x "$CPD" ] || { echo "missing converter binaries"; exit 1; }

# ---- 1. region N6E2 raw output -------------------------------------------------------
[ -f "$OSM" ] || { echo "no OSM input: $OSM"; exit 1; }
#[ -f "$STOCK_DIR/${REGION}AA.IDX" ] || { echo "no stock ${REGION}AA.IDX in $STOCK_DIR"; exit 1; }
rm -rf "$RAW" "$VERIFY"; mkdir -p "$RAW" "$VERIFY" "$LOGS"
log "osm2map: $OSM -> $RAW (this is the RAM-heavy step)"
"$O2M" "$OSM" "$RAW" >"$LOGS/gen.log" 2>&1 || { tail -20 "$LOGS/gen.log"; echo "osm2map FAILED"; exit 1; }
grep -E '^(\[region|L[0-3]:|wrote)' "$LOGS/gen.log" | sed 's/^/    /'

# ---- 2. structural gate ---------------------------------------------------------------
log "tmcheck"
if ! python3 "$REPO/diag/tmcheck.py" "$RAW/${REGION}AA.IDX" >"$LOGS/tmcheck.txt" 2>&1; then
  tail -20 "$LOGS/tmcheck.txt"; echo "TMCHECK FAIL"; exit 1
fi
tail -1 "$LOGS/tmcheck.txt" | sed 's/^/    /'

# ---- 3. pack: CPRNAV_2-compress into the card layout ------------------------------------
rm -rf "$PKG/DATA"; mkdir -p "$PKG/DATA/DATA/MAP" "$PKG/DATA/CONNECT/MAP"
log "cprnav_compress --level 9"
: >"$LOGS/pack.log"
for f in "${FILES[@]}"; do
  [ -f "$RAW/$f" ] || { echo "missing raw $f"; exit 1; }
  "$CPC" "$RAW/$f" "$PKG/DATA/DATA/MAP/$f" --level 9 >>"$LOGS/pack.log" 2>&1
done
# the region .IDX must also ship in CONNECT (region/relation side of the card)
"$CPC" "$RAW/${REGION}AA.IDX" "$PKG/DATA/CONNECT/MAP/${REGION}AA.IDX" --level 9 >>"$LOGS/pack.log" 2>&1
sed 's/^/    /' "$LOGS/pack.log"

# ---- 4. verify: every packaged file decompresses back to the raw source ---------------
log "round-trip verify"
"$CPD" "$PKG/DATA/CONNECT/MAP/${REGION}AA.IDX" "$VERIFY/CONNECT_AA.IDX" >/dev/null
cmp "$VERIFY/CONNECT_AA.IDX" "$RAW/${REGION}AA.IDX"
for f in "${FILES[@]}"; do
  "$CPD" "$PKG/DATA/DATA/MAP/$f" "$VERIFY/$f" >/dev/null
  cmp "$VERIFY/$f" "$RAW/$f"
done
log "verify OK (all packaged files decompress byte-identical)"

# ---- 5. summary -------------------------------------------------------------------------
find "$PKG/DATA" -type f -printf '    %10s  %P\n' | sort -k2
log "deploy pack ready: $PKG/DATA"
log "flash: copy the DATA/ folder over the card's DATA/ (overwrites ${REGION}* only)"
