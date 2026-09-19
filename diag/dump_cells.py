#!/usr/bin/env python3
# Dump raw structure of the first block(s) that contain POLYGON cells (list0 cnt>0) for a region set,
# so an osm2map-generated block can be compared byte-for-byte against a Bosch stock block.
# Usage: dump_cells.py <IDX_dir> [profile_suffix=102] [maxblocks=3]
import struct, os, sys
B32=b"0123456789ABCDEFGHIJKLMNOPQRSTUV"
def u16(d,o): return struct.unpack_from('<H',d,o)[0]
def u32(d,o): return struct.unpack_from('<I',d,o)[0]
def i16(d,o): return struct.unpack_from('<h',d,o)[0]
def suf(rp): v=rp&0xFF; return "1"+chr(B32[v//32])+chr(B32[v%32])

DIR=sys.argv[1]; WANT=int(sys.argv[2],0) if len(sys.argv)>2 else 0x02; MAXB=int(sys.argv[3]) if len(sys.argv)>3 else 3
IDX=os.path.join(DIR,"N6E2AA.IDX"); d=open(IDX,'rb').read()
pt=u16(d,0x14)*4; tables=[]
for i in range(4):
    o=pt+i*12; tables.append((u32(d,o+3)>>8,u32(d,o+7)>>8))
def entries(L,k):
    base=tables[L][1]+k*8; a=u32(d,base); b=u32(d,base+4); lo=a&0xFFFF
    if lo&0x8000: return []
    out=[]
    if lo&0x4000:
        for j in range(a>>16):
            q=b+j*8; aa=u32(d,q)
            if (aa&0xFFFF)&0xC000==0: out.append((aa&0x3FFF,aa>>16,u32(d,q+4)))
    else: out.append((lo&0x3FFF,a>>16,b))
    return out

MAPFN=os.path.join(DIR,"N6E2"+suf(WANT)+".MAP")
m=open(MAPFN,'rb').read()
print(f"{MAPFN}  size={len(m)}")

found=0
for L in range(4):
    for k in range(tables[L][0]):
        for (rp,ln,off) in entries(L,k):
            if (rp & 0xFF)!=WANT: continue
            if off+4>len(m): continue
            ln2=u32(m,off)&0xFFFF; blen=ln2*4
            if off+blen>len(m): continue
            s=[u16(m,off+4+i*4) for i in range(3)]   # list starts
            c=[u16(m,off+6+i*4) for i in range(3)]   # list counts
            if c[0]==0: continue   # want a block with polygons
            print(f"\n== L{L} tile {k}: prof ..{suf(rp)} @{off:#x} len={ln2}w  listStart={s} listCnt={c}")
            for li in range(3):
                st=s[li]; base=st*4
                for ci in range(min(c[li],4)):
                    p=base+ci*12
                    state=u16(m,off+p); feat=u16(m,off+p+2); w3=u16(m,off+p+4); w4=u16(m,off+p+6); t0=u16(m,off+p+8); t1=u16(m,off+p+10)
                    line=f"  li{li} c{ci}: state={state:#06x} feat={feat:#06x} w3(startidx)={w3} w4(cnt)={w4} annDesc=({t0:#04x},{t1:#04x})"
                    # if it's a geometry cell (li<2 and looks like point run), dump first pool pts as words
                    print(line)
                    if li<2 and 0 < w4 <= 6:
                        pts=[(i16(m,off+(w3+pi)*4), i16(m,off+(w3+pi)*4+2)) for pi in range(w4)]
                        print(f"        pool@{w3} dpts={pts}")
            found+=1
            break
        if found>=MAXB: break
    if found>=MAXB: break
print(f"\nblocks-with-polygons shown: {found}")
