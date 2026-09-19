#!/usr/bin/env python3
"""Package a built region out-dir into a head-unit flashable trial tree.

Produces (matching trials/21,22 layout):
    <trial>/DATA/DATA/MAP/<R>1xx.MAP/.TCI + <R>AA.IDX   (cprnav -9 compressed)
    <trial>/DATA/CONNECT/MAP/<R>AA.IDX                  (same compressed IDX)
    <trial>/tmcheck.txt                                  (run on the UNcompressed source)
    <trial>/README.txt                                   (caller-provided --readme or a stub)

With --rnw <osm2rnw out dir> additionally packs the routing half:
    <trial>/DATA/DATA/MAP/<shard>.TCI                    (locator, next to the MAP shards)
    <trial>/DATA/DATA/RNW/CCP/<REGION>/NAV%05u.DAT       (clusters)
    <trial>/rnwcheck.txt                                 (rnwcheck --generated on the source)

Each compressed file is decompressed back and diffed byte-for-byte against the
source to prove the container round-trips (the head-unit reads the compressed form).
NOTE: the SD trial tree only ADDS/OVERWRITES files - stale stock PTH files stay on
the card; our clusters keep flags bit 0x80 clear so they are never .PTH-patched.
Usage: assemble_trial.py <out_dir> <trial_dir> [--readme FILE] [--level 9] [--rnw DIR]
"""
import argparse, os, shutil, subprocess, sys, tempfile

BIN = "/home/marek/Ext/.cargo_cache/release"
COMPRESS = os.path.join(BIN, "cprnav_compress")
DECOMPRESS = os.path.join(BIN, "cprnav_decompress")
HERE = os.path.dirname(os.path.abspath(__file__))


def sh(cmd):
    sys.stderr.write("+ " + " ".join(cmd) + "\n")
    return subprocess.run(cmd, capture_output=True, text=True)


def pack(src, dst, level):
    """compress src -> dst, then decompress and require byte-identity."""
    r = sh([COMPRESS, src, dst, "--level", level])
    if r.returncode != 0:
        sys.stderr.write(r.stderr)
        sys.exit(f"compress failed: {src}")
    with tempfile.NamedTemporaryFile(delete=False) as tf:
        tmp = tf.name
    d = sh([DECOMPRESS, dst, tmp])
    if d.returncode != 0:
        os.unlink(tmp)
        sys.exit(f"decompress failed: {dst}")
    same = os.path.getsize(tmp) == os.path.getsize(src) and \
        open(tmp, 'rb').read() == open(src, 'rb').read()
    os.unlink(tmp)
    if not same:
        sys.exit(f"ROUND-TRIP MISMATCH: {src}")
    print(f"{os.path.basename(src)}\tcompressed OK\tround-trip byte-identical")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out_dir")
    ap.add_argument("trial_dir")
    ap.add_argument("--readme")
    ap.add_argument("--level", default="9")
    ap.add_argument("--rnw", help="osm2rnw output dir (contains MAP/*.TCI + <REGION>/NAV*.DAT)")
    ap.add_argument("--region-ident", default="0x402")
    a = ap.parse_args()

    src = os.path.abspath(a.out_dir)
    files = sorted(os.listdir(src))
    data_map = os.path.join(a.trial_dir, "DATA", "DATA", "MAP")
    conn_map = os.path.join(a.trial_dir, "DATA", "CONNECT", "MAP")
    os.makedirs(data_map, exist_ok=True)
    os.makedirs(conn_map, exist_ok=True)

    # rnwcheck the uncompressed RNW source FIRST (abort before touching the trial tree).
    rnw_out = None
    if a.rnw:
        rnw_out = os.path.abspath(a.rnw)
        regions = [d for d in os.listdir(rnw_out)
                   if os.path.isdir(os.path.join(rnw_out, d))
                   and any(f.upper().startswith("NAV")
                           for f in os.listdir(os.path.join(rnw_out, d)))]
        if not regions:
            sys.exit(f"--rnw dir {rnw_out} has no region subdir")
        region = regions[0]
        rc = sh([sys.executable, os.path.join(HERE, "rnwcheck.py"),
                 os.path.join(rnw_out, "MAP"), os.path.join(rnw_out, region),
                 "--ident", a.region_ident, "--generated"])
        if rc.returncode != 0:
            sys.stderr.write(rc.stdout + rc.stderr)
            sys.exit("rnwcheck FAILED on source; aborting (not packaging a broken RNW build)")

    # tmcheck the uncompressed source (authoritative structural validation).
    tc = sh([sys.executable, os.path.join(HERE, "tmcheck.py"), src])
    if tc.returncode != 0:
        sys.stderr.write(tc.stdout + tc.stderr)
        sys.exit("tmcheck FAILED on source; aborting (not packaging a broken build)")

    idx_name = None
    for f in files:
        s = os.path.join(src, f)
        if not os.path.isfile(s) or os.path.getsize(s) == 0:
            if os.path.getsize(s) == 0:
                sys.stderr.write(f"skip empty {f}\n")
            continue
        pack(s, os.path.join(data_map, f), a.level)
        if f.endswith("AA.IDX"):
            idx_name = f
            shutil.copy2(os.path.join(data_map, f), os.path.join(conn_map, f))

    if rnw_out:
        rnw_map = os.path.join(rnw_out, "MAP")
        for f in sorted(os.listdir(rnw_map)):
            if f.upper().endswith(".TCI"):
                pack(os.path.join(rnw_map, f), os.path.join(data_map, f), a.level)
        for region in regions:
            rd = os.path.join(rnw_out, region)
            dst = os.path.join(a.trial_dir, "DATA", "DATA", "RNW", "CCP", region)
            os.makedirs(dst, exist_ok=True)
            for f in sorted(os.listdir(rd)):
                s = os.path.join(rd, f)
                if os.path.isfile(s) and os.path.getsize(s) > 0:
                    pack(s, os.path.join(dst, f), a.level)
        with open(os.path.join(a.trial_dir, "rnwcheck.txt"), "w") as fh:
            fh.write(rc.stdout)
            fh.write("\n(packaged compressed RNW tree: every file decompressed back and "
                     "verified byte-identical to the validated source; bit 0x80 of cluster "
                     "flags is clear -> stock NAV____N.PTH on the card will not be applied)\n")

    with open(os.path.join(a.trial_dir, "tmcheck.txt"), "w") as fh:
        fh.write(tc.stdout)
        fh.write("\n(packaged compressed tree: every file decompressed back and "
                 "verified byte-identical to the validated source)\n")
    if a.readme and os.path.exists(a.readme):
        shutil.copy2(a.readme, os.path.join(a.trial_dir, "README.txt"))
    print(f"\nTRIAL: {a.trial_dir}")
    print(f"  flash the DATA/ tree. region IDX: {idx_name}"
          + (f" + RNW/{'+'.join(regions)}" if rnw_out else ""))


if __name__ == "__main__":
    main()
