#!/usr/bin/env python3
# Enumerate Bosch TravelMap regions from the stock <REGION>AA.IDX catalog and report their bbox.
# A region's bbox is 4 little-endian u32 at byte offsets 4,8,12,16 in its <REGION>AA.IDX, in
# "PAU" units where degrees = value * 180 / 2^31 (same decode as osm2map setup_region).
#
# Usage:
#   regions.py                     list every region + deg bbox
#   regions.py --intersects W,S,E,N   only regions whose bbox overlaps the given box
#   regions.py --poland            shortcut for Poland's bbox
import sys, os, struct, glob

STOCK = os.environ.get(
    "STOCK_DIR",
    "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/MAP",
)
PAU = (1 << 31) / 180.0

def bbox_of(idx_path):
    with open(idx_path, "rb") as f:
        d = f.read(20)
    if len(d) < 20:
        return None
    w, s, e, n = struct.unpack_from("<4I", d, 4)
    # osm2map setup_region decodes degrees = value / PAU  (i.e. value * 180 / 2^31).
    return (w / PAU, s / PAU, e / PAU, n / PAU)

def main():
    args = sys.argv[1:]
    filt = None
    if "--poland" in args:
        filt = (14.0, 48.9, 24.2, 54.9)
    if "--intersects" in args:
        i = args.index("--intersects")
        filt = tuple(float(x) for x in args[i + 1].split(","))
    rows = []
    for p in sorted(glob.glob(os.path.join(STOCK, "*AA.IDX"))):
        name = os.path.basename(p)[: -len("AA.IDX")]
        b = bbox_of(p)
        if b is None:
            continue
        rows.append((name, b))
    if filt:
        fw, fs, fe, fn = filt
        rows = [
            (nm, b)
            for nm, b in rows
            if not (b[2] < fw or b[0] > fe or b[3] < fs or b[1] > fn)
        ]
    for nm, (w, s, e, n) in rows:
        print(f"{nm}\t{w:.3f}\t{s:.3f}\t{e:.3f}\t{n:.3f}")
    sys.stderr.write(f"{len(rows)} regions\n")

if __name__ == "__main__":
    main()
