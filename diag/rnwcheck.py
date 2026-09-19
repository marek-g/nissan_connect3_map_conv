#!/usr/bin/env python3
# RNW "car-parity" validator (companion to tmcheck.py) for shard .tci + NAV inventory.
#   .tci  header {u16 0, u16 92, u32 size, u16 partOff(0x84), u16 partCnt(4)}; per-level tile
#         tables of TCITile{nPrim<=nAll, clusterListOff}; refs pool of
#         TCIClusterId {u32 (off&~0x3FFF)|regionIdent, u16 length, u16 fileId} (order fixed
#         2026-09, DAPIAPP disasm @0x8deb20).
#   refs  must in-bounds in NAV%05u.DAT, carry the expected regionIdent, length>0, and the
#         cluster's flags byte cluster[+2] keeps bit 0x80 CLEAR in OUR data - set ->
#         u16PatchCluster @0x90ab30 applies NAV____<b&7>.PTH from CONNECT over the cluster
#         (stale POL patches exist). NOTE: STOCK shards legitimately have zero-ref padding,
#         table-region garbage after the last filled tile (the ref pool read as entries), and
#         0x80-flagged (officially patchable) clusters - those are COUNTED there only.
#         Pass --generated (our osm2rnw output) to make them all hard violations.
# Usage:
#   rnwcheck.py <file.tci|dir-with-tci> [region-dir-with-NAV*.DAT] [--ident 0x402] [--generated]
# Exit 0 = clean, 1 = violations found, 2 = usage/parse error.
import struct, sys, os, glob

def u32(d, o): return struct.unpack_from('<I', d, o)[0]

def nav_inventory(region_dir):
    inv = {}
    for f in glob.glob(os.path.join(region_dir, "NAV[0-9][0-9][0-9][0-9][0-9].DAT")):
        try: inv[int(os.path.basename(f)[3:8])] = f
        except ValueError: pass
    return inv

def check_tci(path, ident, inv, generated):
    d = open(path, "rb").read()
    name = os.path.basename(path)
    viol, stats = [], {"tiles": 0, "refs": 0, "zero": 0, "pth": 0, "missing": 0,
                       "oob": 0, "garbage": 0, "badident": 0, "zlen": 0, "badprim": 0,
                       "bigoff": 0}
    cache = {}
    def flag(key, msg):
        stats[key] += 1
        if generated and stats[key] <= 20:
            viol.append(f"{name}: {msg}")
    if len(d) < 0xb4:
        return viol + [f"{name}: too small for header"], stats
    f0, f1, size, partOff, partCnt = struct.unpack_from('<HHIHH', d, 0)
    if f1 != 92: viol.append(f"{name}: f1={f1} != 92 (not a shard TCI?)")
    if size != len(d): viol.append(f"{name}: header size {size} != file {len(d)}")
    if partOff < 0x84 or partOff + partCnt * 12 > len(d):
        return viol + [f"{name}: part table @{partOff} x{partCnt} out of file"], stats
    for i in range(partCnt):
        lvl, _, maxTile, off = struct.unpack_from('<B3sIH', d, partOff + 12 * i)
        if lvl != i: viol.append(f"{name}: partition {i} claims level {lvl}")
        if maxTile == 0 or off + maxTile * 8 > len(d):
            viol.append(f"{name}: lvl{lvl} table [{off}..+{maxTile}*8) past EOF"); continue
        for t in range(maxTile):
            nPrim, nAll, clo = struct.unpack_from('<HHI', d, off + 8 * t)
            if nAll == 0 and nPrim == 0: continue
            if nPrim > nAll:  # garbage: pool bytes read past the last real tile
                flag("badprim", f"lvl{lvl} tile {t}: nPrim {nPrim} > nAll {nAll}")
                continue
            if clo + 8 * nAll > len(d):
                flag("garbage", f"lvl{lvl} tile {t}: pool [{clo}..+{8*nAll}) past EOF")
                continue
            stats["tiles"] += 1
            for r in range(nAll):
                offv, ln, fid = struct.unpack_from('<IHH', d, clo + 8 * r)
                stats["refs"] += 1
                if offv == 0:
                    flag("zero", f"lvl{lvl} tile {t} ref {r}: zero ref with nAll>0")
                    continue
                got = offv & 0x3FFF
                if ident is not None and got != ident:
                    flag("badident", f"lvl{lvl} tile {t} ref {r}: ident {got:#x} != {ident:#x}")
                if offv >= (1 << 28):
                    flag("bigoff", f"tile {t} ref {r}: packed {offv:#x} >= 2^28 (reader rejects)")
                if ln == 0:
                    flag("zlen", f"tile {t} ref {r}: zero length"); continue
                f = inv.get(fid) if inv else None
                if inv is not None and f is None:
                    flag("missing", f"tile {t} ref {r}: NAV{fid:05d}.DAT not in region dir")
                    continue
                if f is not None:
                    m = cache.get(f)
                    if m is None: cache[f] = m = open(f, "rb").read()
                    base = offv & 0xFFFFC000
                    if base + ln > len(m):
                        flag("oob", f"tile {t} ref {r}: [{base:#x}..+{ln}) past NAV{fid:05d} ({len(m)})")
                    elif m[base + 2] & 0x80:
                        flag("pth", f"NAV{fid:05d}+{base:#x}: flags {m[base+2]:#04x} bit0x80 -> .PTH applied")
    return viol, stats

def popcount(v): return bin(v).count("1")

def scan_ci(inv, ident, generated):
    """NAV files in the region dir: ci1/ci2 (24B nav_tclClusterInfo records @0x8910cc)
    must carry the packed regionIdent in the fileOffset low 14 bits."""
    viol, stats = [], {"clusters": 0, "ci": 0}
    mism = 0
    for f, p in sorted(inv.items()):
        d = open(p, "rb").read()
        for off in range(0x4000, len(d), 0x4000):
            if off + 0x1a > len(d): break
            refLon, refLat = struct.unpack_from('<ii', d, off + 8)
            shift = d[off + 0x10]
            ooff, ocnt, lf = struct.unpack_from('<HHH', d, off + 0x12)
            if not (0x1a <= ooff < 0x8000 and 1 <= ocnt <= 4000 and shift <= 20): continue
            if not (-180 * 10**7 <= refLon <= 180 * 10**7 and -90 * 10**7 <= refLat <= 90 * 10**7): continue
            desc = off + ooff + 4 * ocnt
            bits = [b for b in range(11) if lf >> b & 1]
            if desc + 4 * len(bits) > len(d): continue
            stats["clusters"] += 1
            for i, bit in enumerate(bits):
                so, cnt = struct.unpack_from('<HH', d, desc + 4 * i)
                if bit not in (2, 3) or not (0 < cnt < 4000): continue
                for j in range(cnt):
                    q = off + so + 24 * j
                    if q + 24 > len(d): break
                    foff, ln, fid = struct.unpack_from('<IHH', d, q)
                    if foff == 0 or ln == 0: continue
                    stats["ci"] += 1
                    got = foff & 0x3FFF
                    if ident is not None and got != ident:
                        mism += 1
                        if len(viol) < 5:
                            viol.append(f"{os.path.basename(p)}+{off:#x}: ci[{bit}#{j}] ident {got:#x} != {ident:#x}")
    if mism > 5:
        viol.append(f"... {mism-5} more ci ident mismatches (total {mism})")
    return viol, stats

def check_navroot(path, ident, generated):
    d = open(path, "rb").read()
    hdr, fsize = struct.unpack_from("<II", d, 0)
    ro, rc, ao, ac = struct.unpack_from("<HHHH", d, 0xC)
    if fsize != len(d) or not (0 < ro <= ro + 24 * rc <= ao < hdr):
        return [f"header {hdr:#x}/{fsize:#x} vs {len(d):#x}, roots@{ro:#x}x{rc}, annots@{ao:#x}"]
    p = ro + 24 * rc
    for i in range(rc):
        o, cid, fid, _, _, _, _, so, oc, fl = struct.unpack_from("<IHHiiBBHHH", d, ro + 24 * i)
        if so != p or fl != 0x200:
            return [f"root {i}: shapeOff {so:#x} != running {p:#x} or flags {fl:#x}"]
        p += 4 * oc
    if p != ao:
        return [f"shape chain ends {p:#x}, annots start {ao:#x}"]
    q = ao
    for _ in range(ac):
        ln, ty = struct.unpack_from("<HH", d, q)
        if ln < 4 or q + ln > hdr:
            return ["annotation TLV walks past header record"]
        q += ln
    viol = []
    if generated:
        for i in range(rc):
            o = struct.unpack_from("<I", d, ro + 24 * i)[0]
            if ident is not None and (o & 0x3FFF) != ident:
                viol.append(f"root {i}: ident {o & 0x3FFF:#x} != region ident")
    return viol


def main():
    args = list(sys.argv[1:])
    ident, generated = None, False
    if "--ident" in args:
        i = args.index("--ident"); ident = int(args[i + 1], 0); del args[i:i + 2]
    if "--generated" in args:
        args.remove("--generated"); generated = True
    if not args:
        print(__doc__.split("Usage:")[1]); sys.exit(2)
    target = args[0]
    region_dir = args[1] if len(args) > 1 else None
    tci_files = [target] if os.path.isfile(target) else sorted(glob.glob(os.path.join(target, "*.TCI")))
    if not tci_files:
        print(f"no .TCI files at {target}"); sys.exit(2)
    inv = nav_inventory(region_dir) if region_dir else None
    if region_dir:
        print(f"region dir: {region_dir} ({len(inv)} NAV files)")
        cviol, cst = scan_ci(inv, ident, generated)
        line = f"NAV inventory: clusters={cst['clusters']} ci-refs={cst['ci']}"
        if generated and cviol:
            print(line + " CI-IDENT VIOLATIONS:")
            for v in cviol[:10]: print("  " + v)
            sys.exit(1)
        if cviol:
            print(line + f" (info: ci-ident mismatches, e.g. {cviol[0]})")
        else:
            print(line + " ci-ident OK")
        nr = os.path.join(region_dir, "NAV_ROOT.DAT")
        if os.path.exists(nr):
            nviol = check_navroot(nr, ident, generated)
            print(f"NAV_ROOT.DAT: roots/annots re-parse " + ("OK" if not nviol else "VIOLATIONS:"))
            for v in nviol[:10]: print("  " + v)
            if nviol and generated:
                sys.exit(1)
    bad = 0
    for p in tci_files:
        viol, st = check_tci(p, ident, inv, generated)
        name = os.path.basename(p)
        soft = {k: v for k, v in st.items() if v and k not in ("tiles", "refs")}
        print(f"{name}: tiles={st['tiles']} refs={st['refs']} " +
              " ".join(f"{k}={v}" for k, v in soft.items()) + " " +
              ("OK" if not viol else f"VIOLATIONS={len(viol)}"))
        for v in viol[:40]: print("  " + v)
        if len(viol) > 40: print(f"  ... {len(viol)-40} more")
        if viol: bad += 1
    sys.exit(1 if bad else 0)

main()
