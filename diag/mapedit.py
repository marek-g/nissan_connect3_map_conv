#!/usr/bin/env python3
# TravelMap MAP->MAP editor: shift geometry near a location by a ground offset.
# Edits ONLY stored i16 coordinate deltas (block sizes/offsets/layout untouched),
# so the stock N6E2AA.IDX still resolves every block. Mirrors map2osm decode.
import struct, sys, os, math

SHIFTS=[13,10,7,4]
B32=b"0123456789ABCDEFGHIJKLMNOPQRSTUV"
TILECNT=[1,25,2500,250000]
PAU=(1<<31)/180.0

def u16(d,o): return struct.unpack_from('<H',d,o)[0]
def u32(d,o): return struct.unpack_from('<I',d,o)[0]
def i16(d,o): return struct.unpack_from('<h',d,o)[0]
def prof_suffix(rp):
    v=rp&0xFF; return "1"+chr(B32[v//32])+chr(B32[v%32])

class Region:
    def __init__(self,idx):
        d=open(idx,'rb').read(); self.d=d
        self.w=u32(d,4); self.s=u32(d,8); self.e=u32(d,12); self.n=u32(d,16)
        pt=u16(d,0x14)*4; self.tables=[]
        for i in range(4):
            o=pt+i*12; a=u32(d,o+3); b=u32(d,o+7); self.tables.append((a>>8,b>>8))
    def entries(self,L,k):
        base=self.tables[L][1]+k*8; d=self.d; a=u32(d,base); b=u32(d,base+4); lo=a&0xFFFF
        if lo&0x8000: return []
        out=[]
        if lo&0x4000:
            for j in range(a>>16):
                q=b+j*8; aa=u32(d,q)
                if (aa&0xFFFF)&0xC000==0: out.append((aa&0x3FFF, aa>>16, u32(d,q+4)))
        else:
            out.append((lo&0x3FFF, a>>16, b))
        return out
    def tile_extent(self,L,k):
        w=self.e-self.w; h=self.n-self.s
        if L==0: rw,rs,re,rn=0,0,w,h
        elif L==1: c=k%5;r=k//5;rw,rs,re,rn=w*c//5,h*r//5,w*(c+1)//5,h*(r+1)//5
        elif L==2: p=k//100;t=k%100;col=(p%5)*10+(t%10);row=(p//5)*10+(t//10);rw,rs,re,rn=w*col//50,h*row//50,w*(col+1)//50,h*(row+1)//50
        else: p=k//10000;s=(k//100)%100;t=k%100;col=(p%5)*100+(s%10)*10+(t%10);row=(p//5)*100+(s//10)*10+(t//10);rw,rs,re,rn=w*col//500,h*row//500,w*(col+1)//500,h*(row+1)//500
        a=SHIFTS[L]+1; al=lambda x:(x>>a)<<a
        return (al(self.w+rw),al(self.s+rs),al(self.w+re),al(self.s+rn))
    def center(self,L,k):
        w2,s2,e2,n2=self.tile_extent(L,k); return ((w2+e2)//2,(s2+n2)//2)

def clamp16(v): return 32767 if v>32767 else (-32768 if v<-32768 else v)
def rshift(x,sh):
    # round(x / 2**sh)
    return (x + (1<<(sh-1)))>>sh if x>=0 else -(((-x) + (1<<(sh-1)))>>sh)

def process_block(fd, off, sh, cx, cy, box, dd_x, dd_y):
    ln=u32(fd,off)&0xFFFF; blen=ln*4; changed=0
    for li in range(3):
        st=u16(fd,off+4+li*4); cnt=u16(fd,off+6+li*4)
        if cnt==0: continue
        for ci in range(cnt):
            p=st*4+ci*12
            if p+12>blen: break   # mirror map2osm blk-relative bound; never touch neighbor block
            feat=u16(fd,off+p+2); w3=u16(fd,off+p+4); w4=u16(fd,off+p+6)
            if li<2:
                pidx=w3; cnt2=w4
                if pidx*4+cnt2*4 > ln*4: continue
                for pi in range(cnt2):
                    wo=pidx+pi; a=off+wo*4
                    dlon=i16(fd,a); dlat=i16(fd,a+2)
                    lo=cx+(dlon<<sh); la=cy+(dlat<<sh)
                    if box(lo,la):
                        struct.pack_into('<h',fd,a,clamp16(dlon+dd_x))
                        struct.pack_into('<h',fd,a+2,clamp16(dlat+dd_y)); changed+=1
            else:
                if feat&0xF000==0xF000: continue
                dlon=w3-65536 if w3>=32768 else w3; dlat=w4-65536 if w4>=32768 else w4
                if dlon==0 and dlat==0: continue
                lo=cx+(dlon<<sh); la=cy+(dlat<<sh)
                if box(lo,la):
                    struct.pack_into('<h',fd,off+p+4,clamp16(dlon+dd_x))
                    struct.pack_into('<h',fd,off+p+6,clamp16(dlat+dd_y)); changed+=1
    return changed

def main():
    idx,mapdir,outdir=sys.argv[1],sys.argv[2],sys.argv[3]
    lon,lat=float(sys.argv[4]),float(sys.argv[5])
    dE=float(sys.argv[6]); dN=float(sys.argv[7]); Rm=float(sys.argv[8])
    cxp=round(lon*PAU); cyp=round(lat*PAU)
    cosl=math.cos(math.radians(lat))
    dLon_pau=round((dE/(111320.0*cosl))*PAU)
    dLat_pau=round((dN/111320.0)*PAU)
    halfLon=(Rm/(111320.0*cosl))*PAU; halfLat=(Rm/111320.0)*PAU
    wmin=cxp-halfLon; emin=cxp+halfLon; smin=cyp-halfLat; nmax=cyp+halfLat
    box=lambda lo,la: wmin<=lo<=emin and smin<=la<=nmax
    print(f"shift PAU lon={dLon_pau} lat={dLat_pau}; window halfLonPAU={int(halfLon)} halfLatPAU={int(halfLat)}")
    R=Region(idx); os.makedirs(outdir,exist_ok=True)
    files={}; changed={}
    for L in range(4):
        dd_x=rshift(dLon_pau,SHIFTS[L]); dd_y=rshift(dLat_pau,SHIFTS[L])
        if dd_x==0 and dd_y==0:   # coarse level, sub-precision shift; still move if any rounding gives 0 -> skip silently
            pass
        touched=0
        for k in range(TILECNT[L]):
            e=R.tile_extent(L,k)
            if e[2]<wmin or e[0]>emin or e[3]<smin or e[1]>nmax: continue
            cx,cy=R.center(L,k); sh=SHIFTS[L]
            for (rp,ln,off) in R.entries(L,k):
                fn=f"N6E2{prof_suffix(rp)}.MAP"
                if fn not in files:
                    src=os.path.join(mapdir,fn)
                    if not os.path.exists(src): continue
                    files[fn]=bytearray(open(src,'rb').read()); changed[fn]=0
                if fn in files:
                    changed[fn]+=process_block(files[fn],off,sh,cx,cy,box,dd_x,dd_y); touched+=1
        print(f"L{L}: shift/stored=({dd_x},{dd_y}) tiles_in_window_blocks={touched}")
    dirties=[f for f in changed if changed[f]>0]
    for fn in dirties:
        open(os.path.join(outdir,fn),'wb').write(files[fn])
    print("edited files:", {f:changed[f] for f in dirties} or "NONE")

if __name__=='__main__': main()
