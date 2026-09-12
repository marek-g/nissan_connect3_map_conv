#!/usr/bin/env python3
"""Enumerate Bosch TravelMap regions from the stock (decompressed) MAP directory and emit a
JSON manifest driving the per-region generation pipeline.

For every region <NAME> (form  N|S<row>E|W<col>) found in STOCK_DIR we record:
  - bbox W/S/E/N in PAU (deg * 2^31 / 180, as stored little-endian) and in degrees,
  - partOff (u16 @0x14; prefix length = partOff*4 = the region's stock IDX descriptive prefix),
  - the MAP profiles shipped for that region (profile id decoded from the `<NAME>1<b32>` file
    suffix) with each profile file's byte size,
  - a land/hydro classification: a region is "land" iff it ships a profile other than 0x12 ("0I",
    the hydrography/ocean overlay). Land profiles carry roads/POI/land-use — i.e. real coverage.

Only regions already present here may be regenerated in place (the signed MAPWORLD/root files list
them; adding a NEW region is out of reach). So this manifest IS the complete set of legal targets.

Usage:  mk_manifest.py [STOCK_DIR] [-o regions.json] [--all]
  default STOCK_DIR = unpacked firmware MAP dir; --all keeps ocean-only regions too.
"""
import json
import os
import struct
import sys

DEF_STOCK = ("/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/"
             "Map_unpacked/CRYPTNAV/DATA/DATA/MAP")
B32 = "0123456789ABCDEFGHIJKLMNOPQRSTUV"  # Bosch base32 (profile id -> 2 chars)
HYDRO_PROF = 0x12  # "0I"


def pau2deg(v):
    d = (v & 0xFFFFFFFF) * 180.0 / (1 << 31)
    return d - 360.0 if d > 180.0 else d


def prof_from_code(code):
    a, b = B32.index(code[0]), B32.index(code[1])
    return a * 32 + b


def main(argv):
    args = [a for a in argv[1:]]
    stock = DEF_STOCK
    out = None
    keep_all = "--all" in args
    if "-o" in args:
        out = args[args.index("-o") + 1]
    pos = [a for a in args if not a.startswith("-") and a != (out or "\0")]
    if pos:
        stock = pos[0]

    regions = {}  # name -> record

    # 1) bboxes + partOff from AA.IDX headers
    for fn in sorted(os.listdir(stock)):
        if not fn.endswith("AA.IDX"):
            continue
        name = fn[:-6]  # strip "AA.IDX"
        p = os.path.join(stock, fn)
        with open(p, "rb") as f:
            hdr = f.read(32)
        if len(hdr) < 24:
            continue
        part_off = struct.unpack_from("<H", hdr, 0x14)[0]
        w, s, e, n = struct.unpack_from("<IIII", hdr, 4)
        regions[name] = {
            "region": name,
            "bbox_pau": {"w": w, "s": s, "e": e, "n": n},
            "bbox_deg": {
                "w": round(pau2deg(w), 4), "s": round(pau2deg(s), 4),
                "e": round(pau2deg(e), 4), "n": round(pau2deg(n), 4),
            },
            "part_off": part_off,
            "idx_prefix_len": part_off * 4,
            "profiles": {},  # id -> {"code","size"}
        }

    # 2) profiles from *1XX.MAP filenames (name = REGION + "1" + 2-char base32)
    for fn in os.listdir(stock):
        if not fn.endswith(".MAP"):
            continue
        stem = fn[:-4]
        if len(stem) < 3 or stem[-3] != "1":
            continue
        name, code = stem[:-3], stem[-2:]
        rec = regions.get(name)
        if rec is None:
            continue
        try:
            pid = prof_from_code(code)
        except ValueError:
            continue
        size = os.path.getsize(os.path.join(stock, fn))
        rec["profiles"][pid] = {"code": code, "size": size}

    # 3) classify + filter
    out_regions = []
    for name in sorted(regions):
        rec = regions[name]
        profs = rec["profiles"]
        land = [pid for pid in profs if pid != HYDRO_PROF and profs[pid]["size"] > 0]
        rec["has_land"] = bool(land)
        rec["land_profile_ids"] = sorted(land)
        # We have only ever EMITTED (and car-validated) profile id 0x02. Regions whose stock set
        # includes 0x02 are the lowest-risk regeneration targets; regions using other ids exercise
        # the unproven "does an un-shipped profile id render?" question (see resinf investigation).
        rec["has_profile_02"] = 0x02 in profs
        rec["profile_ids"] = sorted(profs)
        if rec["has_land"] or keep_all:
            out_regions.append(rec)

    out_regions.sort(key=lambda r: (r["bbox_deg"]["s"], r["bbox_deg"]["w"]))
    land_n = sum(1 for r in out_regions if r["has_land"])
    doc = {
        "stock_dir": os.path.abspath(stock),
        "region_count": len(out_regions),
        "land_region_count": land_n,
        "note": "regenerate only regions listed here (already referenced by signed MAPWORLD/root files)",
        "regions": out_regions,
    }

    text = json.dumps(doc, indent=1)
    if out:
        with open(out, "w") as f:
            f.write(text + "\n")
    lminW = min(r["bbox_deg"]["w"] for r in out_regions if r["has_land"])
    lmaxE = max(r["bbox_deg"]["e"] for r in out_regions if r["has_land"])
    lminS = min(r["bbox_deg"]["s"] for r in out_regions if r["has_land"])
    lmaxN = max(r["bbox_deg"]["n"] for r in out_regions if r["has_land"])
    sys.stderr.write(
        f"regions kept: {len(out_regions)} (land {land_n})\n"
        f"land extent: W{lminW:.2f} S{lminS:.2f} E{lmaxE:.2f} N{lmaxN:.2f}\n"
    )
    if not out:
        print(text)


if __name__ == "__main__":
    main(sys.argv)
