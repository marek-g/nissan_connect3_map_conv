#!/usr/bin/env python3
# TravelMap structural / "car-parity" validator. A directory with N6E2AA.IDX (+ MAPs (+TCI)).
# Checks the invariants a STRICT parser (the head unit renderer) relies on and that our tolerant
# map2osm reader masks: block magic==len; slot/sub-entry offsets land inside their MAP file; every
# annotation descriptor, run and text record stays INSIDE its own block. A violation here is the
# signature of a firmware SIGSEGV / reboot-loop (out-of-block pointer walked off the end).
import struct, sys, os, glob

SHIFTS=[13,10,7,4]
B32=b"0123456789ABCDEFGHIJKLMNOPQRSTUV"
def u16(d,o): return struct.unpack_from('<H',d,o)[0]
def u32(d,o): return struct.unpack_from('<I',d,o)[0]
def suf(rp): v=rp&0xFF; return "1"+chr(B32[v//32])+chr(B32[v%32])

class R:
    def __init__(s,d):
        s.d=d; s.binoff=u16(d,0); s.pt=u16(d,0x14)*4
        s.tables=[]
        for i in range(4):
            o=s.pt+i*12; s.tables.append((u32(d,o+3)>>8,u32(d,o+7)>>8))
    def entries(s,L,k):
        base=s.tables[L][1]+k*8; d=s.d; a=u32(d,base); b=u32(d,base+4); lo=a&0xFFFF
        if lo&0x8000: return []
        out=[]
        if lo&0x4000:
            for j in range(a>>16):
                q=b+j*8; aa=u32(d,q)
                if (aa&0xFFFF)&0xC000==0: out.append((aa&0x3FFF,aa>>16,u32(d,q+4)))
        else: out.append((lo&0x3FFF,a>>16,b))
        return out

def check_block(name, m, off, viol, exp_len=None):
    if off+4>len(m): viol.append(f"{name}: block off {off} past EOF"); return
    marker=u32(m,off); ln=marker&0xFFFF; blen=ln*4
    # (Ghidra u16Convert) the read window is sized from the SLOT length (MemBlockDesc.len), not the
    # in-block marker; the 3 list headers are read right after skipping this u32. A mismatch means the
    # accessor covers a different span than the block occupies -> truncated/garbage tile or OOB reads.
    if (marker>>16)!=0xFFFF:
        viol.append(f"{name}: marker hi != 0xFFFF (got {marker>>16:#06x} @off {off})")
    if exp_len is not None and ln!=exp_len:
        viol.append(f"{name}: block marker len {ln}w != slot length {exp_len}w (accessor window mismatch)")
    if off+blen>len(m): viol.append(f"{name}: block [{off}..{off+blen}) past EOF (file {len(m)})")
    for li in range(3):
        st=u16(m,off+4+li*4); cnt=u16(m,off+6+li*4)
        if cnt==0: continue
        base=st*4
        for ci in range(cnt):
            p=base+ci*12
            if p+12>blen: viol.append(f"{name}: cell run past block li={li} p={p} blen={blen}"); return
            desc=(u16(m,off+p+10)<<16)|u16(m,off+p+8)
            ast=desc&0xFFFF; acnt=(desc>>16)&0xFFFF
            if acnt==0: continue
            if acnt>4096: viol.append(f"{name}: annot count {acnt} too big"); continue
            ap=ast*4
            for _ in range(acnt):
                if ap+2>blen: viol.append(f"{name}: annot walk past block (pos {ap} blen {blen})"); break
                sz=m[off+ap]; ty=m[off+ap+1]
                if sz<3 or sz>64: viol.append(f"{name}: bad annot size {sz} type {ty:#x} @pos {ap}"); break
                # text pointer types resolve a record INSIDE the block; check its header stays in-block
                if ty==0x7A and ap+4<=blen:
                    v=u16(m,off+ap+2)
                    rp_=v*4
                    if rp_+2>blen: viol.append(f"{name}: 0x7A text ptr {v}w out of block (rp={rp_} blen {blen})")
                    else:
                        n=m[off+rp_]
                        if not (1<=n<=32): viol.append(f"{name}: text rec bad n={n} @rp={rp_}")
                        elif rp_+1+2*n>blen: viol.append(f"{name}: text rec header past block @rp={rp_} n={n}")
                ap+=sz

def main():
    d=sys.argv[1]
    if d.endswith('.IDX'):
        idxp=d
    else:
        idxp=os.path.join(d,'N6E2AA.IDX')
        if not os.path.exists(idxp):
            cand=sorted(glob.glob(os.path.join(d,'*AA.IDX')))
            if len(cand)==1: idxp=cand[0]
            elif cand: print(f"ambiguous IDX in {d}: {[os.path.basename(c) for c in cand]}"); sys.exit(2)
    if not os.path.exists(idxp): print("no IDX at",idxp); sys.exit(2)
    base=os.path.dirname(idxp)
    pref=os.path.basename(idxp)[:-len("AA.IDX")] if idxp.endswith("AA.IDX") else os.path.basename(idxp)[:-4]
    Rg=R(open(idxp,'rb').read()); idxd=Rg.d; nfiles=len(idxd)
    viol=[]
    if Rg.tables[0][1]!=Rg.binoff: viol.append(f"IDX tables[0]={Rg.tables[0][1]:#x} != binOff={Rg.binoff:#x}")
    mcache={}
    def mapdata(rp):
        fn=os.path.join(base,pref+suf(rp)+".MAP")
        data=mcache.get(fn)
        if data is None:
            if not os.path.exists(fn): return None,fn
            data=open(fn,'rb').read(); mcache[fn]=data
        return data,fn
    nblocks=0; nenc=0
    # Scan EVERY slot at every level (no index cap). Validate the regProf ENCODING of all slots
    # and block-check only real data/sub entries. This catches both out-of-block pointers AND the
    # empty/multi encoding that stock fixes to exact bit patterns.
    for L in range(4):
        cnt=Rg.tables[L][0]; taboff=Rg.tables[L][1]
        for k in range(cnt):
            baseo=taboff+k*8
            a=u32(idxd,baseo); b=u32(idxd,baseo+4); lo=a&0xFFFF; length=a>>16
            if (lo|length|b)==0:  # all-zero slot (stock never emits these, but harmless)
                continue
            if lo&0x8000:
                # EMPTY: stock uses EXACTLY regProf=0x8000,len=0,off=0. Any profile bits make the
                # reader resolve a real MAP file at that offset and parse non-block bytes as cells.
                if not(lo==0x8000 and length==0 and b==0):
                    viol.append(f"L{L}/{k}: EMPTY slot malformed regProf={lo:#06x} len={length} off={b:#x} (stock requires 0x8000/0/0)")
                nenc+=1; continue
            if lo&0x4000:
                # MULTI header: bit14 only, profile bits must be cleared (=0x4000), count>0.
                if (lo & ~0x4000)!=0:
                    viol.append(f"L{L}/{k}: MULTI header carries profile bits regProf={lo:#06x} (stock=0x4000)")
                nenc+=1
                if length==0 or b==0 or b+length*8>nfiles:
                    viol.append(f"L{L}/{k}: MULTI sub-list out of IDX (cnt={length} ptr={b:#x} filesz={nfiles})"); continue
                if length>15:
                    viol.append(f"L{L}/{k}: MULTI sub-entry count {length} > 15 cap")
                for j in range(length):
                    q=b+j*8; aa=u32(idxd,q); s_off=u32(idxd,q+4); rp=aa&0xFFFF; ln=aa>>16
                    if (rp&0xC000)!=0: viol.append(f"L{L}/{k}/sub{j}: sub-entry has flag bits set regProf={rp:#06x}"); continue
                    data,fn=mapdata(rp)
                    if data is None: viol.append(f"L{L}/{k}/sub{j}: missing {os.path.basename(fn)}"); continue
                    if rp==0 or ln==0: viol.append(f"L{L}/{k}/sub{j}: zero rp/len"); continue
                    check_block(f"L{L}/{k}/sub{j} {os.path.basename(fn)}@{s_off:#x}", data, s_off, viol, ln); nblocks+=1
                continue
            # DATA slot: regProf must select a real profile file; offset must be past the header.
            if (lo&0xC000)!=0: viol.append(f"L{L}/{k}: DATA slot has flag bits set regProf={lo:#06x}"); nenc+=1; continue
            nenc+=1
            data,fn=mapdata(lo)
            if data is None: viol.append(f"L{L}/{k}: missing {os.path.basename(fn)} for regProf={lo:#06x}"); continue
            if length==0: viol.append(f"L{L}/{k}: DATA len=0")
            check_block(f"L{L}/{k} {os.path.basename(fn)}@{b:#x}", data, b, viol, length); nblocks+=1
    print(f"validated slots: {nenc}  block-checked blocks: {nblocks}")
    if viol:
        print(f"FAIL: {len(viol)} violations; first 25:")
        for v in viol[:25]: print("  ",v)
        sys.exit(1)
    print("PASS: every slot encoding matches stock bit-patterns; all block/slot/annot/text pointers self-contained")

if __name__=='__main__': main()
