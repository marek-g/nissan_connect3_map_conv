#!/usr/bin/env bash
# Bulk region-generation pipeline: manifest -> per-region OSM extract -> osm2map -> tmcheck gate ->
# CPRNAV-2 deploy pack. Parallel (xargs -P) and resumable (per-region .done stamp).
#
#   gen/pipeline.sh [options]
#
# Options (env vars also accepted):
#   --manifest F   regions.json                    (default gen/regions.json)
#   --src PATH     OSM .pbf file OR a directory of .pbf to extract each region from (REQUIRED for a
#                  cold run; e.g. a whole europe-latest.osm.pbf, or a dir of geofabrik country files).
#                  If a pre-made extract WORK/region_<REG>.osm already exists it is used as-is.
#   --out DIR      deploy output root               (default gen/out)
#   --jobs N       parallel regions                 (default nproc)
#   --regions "A B"  explicit region list           (default: regions shipping profile 0x02 = safe set)
#   --all          process every land region        (uses each region's own primary land profile)
#   --profile ID   force the emitted land profile id for all regions (else 0x02 where available)
#   --force        rebuild regions that already have a .done stamp
#
# Safety model (see osm2map_plan.md "Per-region profile / resinf model"):
#   * only regions in the manifest (already referenced by the signed root/RPI tables) are generated;
#   * profile 0x02 is the only car-validated land profile -> default targets are the 0x02 regions
#     (N6E1, N6E2). --all also emits regions lacking 0x02, each with its OWN largest land profile id
#     (so its signed RPI already declares it) — those are UNVALIDATED on the car: verify before use.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/.." && pwd)"
O2M=${O2M:-/home/marek/Ext/.cargo_cache/release/osm2map}
CPC=${CPC:-/home/marek/Ext/.cargo_cache/release/cprnav_compress}
OSMIUM=${OSMIUM:-osmium}

MANIFEST="${MANIFEST:-$HERE/regions.json}"
OUT="${OUT:-$ROOT/gen/out}"; WORK="$OUT/work"; SRC="${SRC:-}"; JOBS="${JOBS:-$(nproc)}"
PROFILE="${PROFILE:-}"; ALL="${ALL:-0}"; FORCE="${FORCE:-0}"; REGIONS="${REGIONS:-}"

# ---- worker mode: build ONE region ------------------------------------------
profile_for() { # <region> -> hex profile id to emit
  python3 - "$MANIFEST" "$1" "${PROFILE:-}" <<'PY'
import json,sys
man,reg,force=sys.argv[1],sys.argv[2],sys.argv[3]
if force: print(force.replace("0x","")); sys.exit()
d=json.load(open(man)); r=next(x for x in d["regions"] if x["region"]==reg)
if 2 in r["profile_ids"]: print("2"); sys.exit()
pids=r.get("land_profile_ids") or r["profile_ids"]
best=max(pids, key=lambda p: r["profiles"][str(p)]["size"])
print("%x"%best)
PY
}
is02() { python3 - "$MANIFEST" "$1" <<'PY'
import json,sys; d=json.load(open(sys.argv[1])); r=next(x for x in d["regions"] if x["region"]==sys.argv[2]); print("1" if 2 in r["profile_ids"] else "0")
PY
}
bbox_for() { python3 - "$MANIFEST" "$1" <<'PY'
import json,sys; d=json.load(open(sys.argv[1])); r=next(x for x in d["regions"] if x["region"]==sys.argv[2]); b=r["bbox_deg"]; print("%.6f,%.6f,%.6f,%.6f"%(b["w"],b["s"],b["e"],b["n"]))
PY
}

if [ "${1:-}" = "--worker" ]; then
  reg="$2"; prof="$(profile_for "$reg")"; bb="$(bbox_for "$reg")"
  rout="$OUT/regions/$reg"; stamp="$rout/.done"
  [ -f "$stamp" ] && [ "$FORCE" != 1 ] && { echo "$reg: done (skip)"; exit 0; }
  rm -rf "$rout"; mkdir -p "$rout/raw" "$rout/deploy/DATA/DATA/MAP" "$rout/deploy/DATA/CONNECT/MAP"
  osm="$WORK/region_$reg.osm"
  if [ ! -f "$osm" ]; then
    mkdir -p "$WORK"
    if [ -d "$SRC" ]; then inputs=$(shopt -s nullglob; echo "$SRC"/*.osm.pbf "$SRC"/*.pbf); else inputs="$SRC"; fi
    [ -n "$inputs" ] || { echo "$reg: NO OSM_SRC and no $osm" > "$rout/FAILED"; exit 1; }
    # shellcheck disable=SC2086
    "$OSMIUM" extract -b "$bb" -s complete_ways -f osm -o "$osm" $inputs --overwrite > "$rout/osmium.log" 2>&1 \
      || { echo "$reg: osmium extract failed (see osmium.log)" > "$rout/FAILED"; exit 1; }
  fi
  env OSM2MAP_PROFILE="$prof" "$O2M" "$osm" "$rout/raw" --region="$reg" > "$rout/gen.log" 2>&1 \
    || { echo "$reg: osm2map failed (see gen.log)" > "$rout/FAILED"; exit 1; }
  if ! python3 "$ROOT/diag/tmcheck.py" "$rout/raw/${reg}AA.IDX" > "$rout/tmcheck.txt" 2>&1; then
    echo "$reg: TMCHECK FAIL" > "$rout/FAILED"; exit 1
  fi
  land="${reg}1$(python3 -c "b='0123456789ABCDEFGHIJKLMNOPQRSTUV';v=int('$prof',16);print(b[v//32]+b[v%32])")"
  for f in "$land.MAP" "$land.TCI" "${reg}10I.MAP" "${reg}10I.TCI" "${reg}AA.IDX"; do
    [ -f "$rout/raw/$f" ] && "$CPC" "$rout/raw/$f" "$rout/deploy/DATA/DATA/MAP/$f" --level 9 >> "$rout/pack.log" 2>&1 || true
  done
  "$CPC" "$rout/raw/${reg}AA.IDX" "$rout/deploy/DATA/CONNECT/MAP/${reg}AA.IDX" --level 9 >> "$rout/pack.log" 2>&1 || true
  echo "$reg: OK  profile=0x$prof bbox=$bb" > "$stamp"
  echo "$(cat "$stamp")"
  exit 0
fi

# ---- main: parse + select + fan out -----------------------------------------
while [ $# -gt 0 ]; do case "$1" in
  --manifest) MANIFEST="$2"; shift 2;; --src) SRC="$2"; shift 2;; --out) OUT="$2"; shift 2;;
  --jobs) JOBS="$2"; shift 2;; --regions) REGIONS="$2"; shift 2;; --profile) PROFILE="$2"; shift 2;;
  --all) ALL=1; shift;; --force) FORCE=1; shift;;
  *) echo "unknown arg: $1" >&2; exit 2;; esac; done
export MANIFEST ROOT OUT WORK SRC PROFILE FORCE O2M CPC OSMIUM
mkdir -p "$OUT" "$WORK"

if [ -z "$REGIONS" ]; then
  REGIONS=$(python3 - "$MANIFEST" "$ALL" <<'PY'
import json,sys
d=json.load(open(sys.argv[1])); allr=sys.argv[2]=="1"
sel=[r["region"] for r in d["regions"] if allr or r["has_profile_02"]]
print(" ".join(sel))
PY
)
fi
echo "regions : $REGIONS"
echo "src     : ${SRC:-<none; needs WORK/region_<REG>.osm pre-made>}"
echo "out     : $OUT   jobs: $JOBS"
printf '%s\n' $REGIONS | xargs -P "$JOBS" -I{} "$ROOT/gen/pipeline.sh" --worker {}
echo "=== summary ==="
for r in $REGIONS; do
  if [ -f "$OUT/regions/$r/.done" ]; then echo "  OK   $r  $(sed 's/.*profile/profile/' "$OUT/regions/$r/.done")"
  elif [ -f "$OUT/regions/$r/FAILED" ]; then echo "  FAIL $r  $(cat "$OUT/regions/$r/FAILED")"
  else echo "  ??   $r"; fi
done
