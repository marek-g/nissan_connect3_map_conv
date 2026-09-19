#!/usr/bin/env python3
# Multi-region MAP builder. Per Bosch region: extract its bbox from a master OSM .pbf with
# `osmium` (pbf -> region .osm XML in one pass), run the existing streaming osm2map over it
# (region override -> stock IDX/catalog + region bbox), validate with tmcheck, and collect the
# emitted profile files (land 0x02 + hydro 0x0I) into one DATA/DATA/MAP tree (mirrors the trials).
#
# The XML parser is unchanged; memory is bounded per region because each run only sees its region
# extract. Regions are read from the stock <REGION>AA.IDX catalog (same bbox decode as osm2map).
#
# Usage:
#   build_regions.py --poland [--jobs N] [--out DIR] [--master PBF] [--stock DIR]
#   build_regions.py --regions N6E1,N6E2 ...
#   build_regions.py --intersects W,S,E,N ...
import argparse, os, struct, subprocess, sys, shutil, glob
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get("OSM2MAP_BIN", "/home/marek/Ext/.cargo_cache/release/osm2map")
STOCK = os.environ.get(
    "STOCK_DIR",
    "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/MAP",
)
MASTER = os.environ.get(
    "MASTER_PBF", "/home/marek/Ext/reverse_engineering/NissanMaps/OSM-map/poland-260909.osm.pbf"
)
PAU = (1 << 31) / 180.0


def region_bboxes(stock):
    out = {}
    for p in glob.glob(os.path.join(stock, "*AA.IDX")):
        name = os.path.basename(p)[: -len("AA.IDX")]
        with open(p, "rb") as f:
            d = f.read(20)
        w, s, e, n = struct.unpack_from("<4I", d, 4)
        out[name] = (w / PAU, s / PAU, e / PAU, n / PAU)
    return out


def overlaps(b, box):
    return not (b[2] < box[0] or b[0] > box[2] or b[3] < box[1] or b[1] > box[3])


def clip(b, box):
    """intersect region bbox b with box; None if disjoint"""
    if box is None:
        return b
    w = max(b[0], box[0]); s = max(b[1], box[1]); e = min(b[2], box[2]); n = min(b[3], box[3])
    if e <= w or n <= s:
        return None
    return (w, s, e, n)


def run(cmd, env=None):
    sys.stderr.write("+ " + " ".join(cmd[:6]) + (" ...\n" if len(cmd) > 6 else "\n"))
    return subprocess.run(cmd, env=env).returncode


def audit_max_blocks(outd):
    """Max sub-blocks in any single tile slot across the 4 level tables of the produced *AA.IDX."""
    idx = glob.glob(os.path.join(outd, "*AA.IDX"))
    if not idx:
        return None
    d = open(idx[0], 'rb').read()
    u16 = lambda o: struct.unpack_from('<H', d, o)[0]
    u32 = lambda o: struct.unpack_from('<I', d, o)[0]
    pt = u16(0x14) * 4
    mx = 0
    for i in range(4):
        o = pt + i * 12
        tc, toff = u32(o + 3) >> 8, u32(o + 7) >> 8
        for k in range(tc):
            base = toff + k * 8
            a = u32(base); lo = a & 0xFFFF
            if lo & 0x8000:
                continue
            cnt = (a >> 16) if (lo & 0x4000) else 1
            if cnt > mx:
                mx = cnt
    return mx


def build_one(name, region_bbox, clip_bbox, work, out, master, stock, lod):
    rb = os.path.join(work, name)
    os.makedirs(rb, exist_ok=True)
    oxml = os.path.join(rb, name + ".osm")
    outd = os.path.join(rb, "out")
    os.makedirs(outd, exist_ok=True)
    # INPUT filter (osmium): the --clip sub-box when given, else the whole region.
    inbox = clip_bbox if clip_bbox else region_bbox
    in_box = ",".join(f"{v:.6f}" for v in inbox)
    # REGION bbox (osm2map tiling/thresholds): ALWAYS the full stock region bbox, so tiles and
    # the PAU-scaled LOD thresholds stay correct even when the input is a clipped sub-area.
    reg_box = ",".join(f"{v:.6f}" for v in region_bbox)
    # 1) pbf -> region .osm (XML) in one streaming pass; master is region-agnostic.
    rc = run(["osmium", "extract", "-b", in_box, master, "-o", oxml, "--overwrite"])
    if rc != 0:
        return (name, "extract-failed", rc)
    # 2) osm2map as this region (STOCK_DIR gives the AA.IDX catalog template + region bbox).
    #    LOD is REQUIRED for dense regions: the coarse levels' netclass caps [0,1,3,7] mirror stock
    #    (L0=motorway..L3=all) and keep every tile within the stock-observed <=5 sub-blocks/tile.
    env = dict(os.environ, OSM2MAP_REGION=name, STOCK_DIR=stock)
    if lod:
        env["OSM2MAP_LOD"] = "1"
    rc = run([BIN, oxml, outd, reg_box], env=env)
    if rc != 0:
        return (name, "osm2map-failed", rc)
    # 3) validate
    rc = run([sys.executable, os.path.join(HERE, "tmcheck.py"), outd])
    if rc != 0:
        return (name, "tmcheck-FAIL", rc)
    # 4) per-tile density audit vs stock (real cap is <=5 sub-blocks/tile, never 15)
    try:
        mx = audit_max_blocks(outd)
        sys.stderr.write(f"{name}: max sub-blocks/tile = {mx}\n")
    except Exception as e:
        sys.stderr.write(f"{name}: audit skipped ({e})\n")
    status = "PASS"
    # 4) collect region files into the shared DATA tree
    dst = os.path.join(out, "DATA", "DATA", "MAP")
    os.makedirs(dst, exist_ok=True)
    os.makedirs(os.path.join(out, "DATA", "CONNECT", "MAP"), exist_ok=True)
    for f in glob.glob(os.path.join(outd, "*")):
        shutil.copy2(f, dst)
        if f.endswith("AA.IDX"):
            shutil.copy2(f, os.path.join(out, "DATA", "CONNECT", "MAP"))
    return (name, status, 0)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--poland", action="store_true")
    ap.add_argument("--intersects")
    ap.add_argument("--regions")
    ap.add_argument("--jobs", type=int, default=2)
    ap.add_argument("--out", default="/tmp/opencode/multiregion")
    ap.add_argument("--master", default=MASTER)
    ap.add_argument("--stock", default=STOCK)
    ap.add_argument("--clip", help="intersect each region bbox with W,S,E,N (test/partial runs); "
                    "shrinks the osmium INPUT only, the region bbox stays full for tiling")
    ap.add_argument("--no-lod", action="store_true",
                    help="disable per-level LOD netclass caps (NOT recommended for dense regions)")
    a = ap.parse_args()
    lod = not a.no_lod
    clipbox = tuple(float(x) for x in a.clip.split(",")) if a.clip else None
    allb = region_bboxes(a.stock)
    if a.regions:
        names = a.regions.split(",")
    else:
        if a.poland:
            box = (14.0, 48.9, 24.2, 54.9)
        elif a.intersects:
            box = tuple(float(x) for x in a.intersects.split(","))
        else:
            ap.error("need --poland / --intersects / --regions")
        names = [n for n, b in allb.items() if overlaps(b, box)]
    names.sort()
    work = os.path.join(a.out, "work")
    os.makedirs(work, exist_ok=True)
    sys.stderr.write(f"{len(names)} regions: {' '.join(names)}\n")
    results = []
    jobs = []
    for n in names:
        cb = clip(allb[n], clipbox) if clipbox else allb[n]
        if cb is None:
            sys.stderr.write(f"skip {n}: outside --clip\n")
            continue
        jobs.append((n, allb[n], cb))
    with ThreadPoolExecutor(max_workers=a.jobs) as ex:
        futs = [ex.submit(build_one, n, rb, cb, work, a.out, a.master, a.stock, lod)
                for n, rb, cb in jobs]
        for f in futs:
            results.append(f.result())
    for n, st, rc in results:
        print(f"{n}\t{st}")
    if any("PASS" not in st for _, st, _ in results):
        sys.exit(1)


if __name__ == "__main__":
    main()
