#!/usr/bin/env python3
# Per-profile content fingerprint of a (DECOMPRESSED) region set: for each profile file that an
# <REGION>AA.IDX references, report size, block count, per-level slot refs and a cell/feature
# histogram split geometry(li0/li1 = polylines/polygons) vs points(li2 = POI), plus annotation
# types. Works on stock or osm2map-generated raw sets. Usage: prof_audit.py [MAPDIR]  (default stock N6E2)
import struct, os, sys
from collections import Counter, defaultdict
B32 = b"0123456789ABCDEFGHIJKLMNOPQRSTUV"
def u16(d, o): return struct.unpack_from('<H', d, o)[0]
def u32(d, o): return struct.unpack_from('<I', d, o)[0]
def suf(rp): v = rp & 0xFF; return "1" + chr(B32[v//32]) + chr(B32[v%32])

MAPDIR = sys.argv[1] if len(sys.argv) > 1 else "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/MAP"
IDX = os.path.join(MAPDIR, "N6E2AA.IDX")
d = open(IDX, 'rb').read()
pt = u16(d, 0x14)*4; tables = []
for i in range(4):
    o = pt+i*12; tables.append((u32(d, o+3)>>8, u32(d, o+7)>>8))  # (tileCnt, tableOffset)

def entries(L, k):
    base = tables[L][1]+k*8; a = u32(d, base); b = u32(d, base+4); lo = a & 0xFFFF
    if lo & 0x8000: return []
    out = []
    if lo & 0x4000:
        for j in range(a>>16):
            q = b+j*8; aa = u32(d, q)
            if (aa & 0xFFFF) & 0xC000 == 0: out.append((aa & 0x3FFF, aa>>16, u32(d, q+4)))
    else: out.append((lo & 0x3FFF, a>>16, b))
    return out

TILECNT = [tables[L][0] for L in range(4)]
refs = defaultdict(set); levels = defaultdict(Counter)
for L in range(4):
    for k in range(TILECNT[L]):
        for (rp, ln, off) in entries(L, k):
            refs[rp].add((off, ln)); levels[rp][L] += 1

cache = {}
def mapdata(rp):
    fn = os.path.join(MAPDIR, "N6E2"+suf(rp)+".MAP")
    if fn not in cache:
        cache[fn] = open(fn, 'rb').read() if os.path.exists(fn) else None
    return cache[fn], os.path.basename(fn)

print(f"{'prof':>5} {'file':>14} {'size':>9} {'blocks':>7} {'slotRefs':>8}  levels(L0..L3)")
for rp in sorted(refs):
    data, fn = mapdata(rp); nb = len(refs[rp]); size = len(data) if data else -1
    lv = "".join(str(levels[rp].get(x, 0)).rjust(7) for x in range(4))
    print(f"  {suf(rp)[-2:]:>3} {fn:>14} {size:>9} {nb:>7} {sum(levels[rp].values()):>8}  {lv}")

print("\nPer-profile cell/feature fingerprint (top codes):\n")
for rp in sorted(refs):
    data, _ = mapdata(rp)
    if not data: continue
    feat_geo = Counter(); feat_pt = Counter(); ncells = 0; annot = Counter()
    for (off, ln) in refs[rp]:
        blen = ln*4
        if off+blen > len(data): continue
        for li in range(3):
            st = u16(data, off+4+li*4); cnt = u16(data, off+6+li*4)
            for ci in range(cnt):
                p = st*4+ci*12
                if p+12 > blen: break
                feat = u16(data, off+p+2); ncells += 1
                (feat_pt if li == 2 else feat_geo)[feat] += 1
                desc = (u16(data, off+p+10) << 16) | u16(data, off+p+8)
                ast = desc & 0xFFFF; acnt = (desc >> 16) & 0xFFFF; ap = ast*4
                for _ in range(min(acnt, 256)):
                    if ap+2 > blen: break
                    sz = data[off+ap]; ty = data[off+ap+1]
                    if sz < 3 or sz > 64: break
                    annot[ty] += 1; ap += sz
    gsum = sum(feat_geo.values()); psum = sum(feat_pt.values())
    print(f"  profile ..{suf(rp)}  cells={ncells} geo={gsum} points/POI={psum}")
    if feat_geo: print("     geo feats :", ", ".join(f"{c:#04x}:{n}" for c, n in feat_geo.most_common(12)))
    if feat_pt:  print("     POI feats :", ", ".join(f"{c:#04x}:{n}" for c, n in feat_pt.most_common(16)))
    if annot:    print("     annots    :", ", ".join(f"{c:#02x}:{n}" for c, n in annot.most_common(12)))
