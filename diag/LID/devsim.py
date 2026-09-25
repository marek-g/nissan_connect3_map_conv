import struct, sys

# faithful port of DAPIAPP.OUT NLAsfBlock decoder (SetDescription + Decode per kind)
# descriptor (in-block): {kind u16, code u16, off u32, param u32} ; member = {off,len=span,count=param,kind,code}

# device: tag -> (n_values, n_bits)
S9 = {1:(28,1),2:(14,2),3:(9,3),4:(7,4),5:(5,5),6:(4,7),7:(3,9),8:(2,14),9:(1,28)}

class Fail(Exception): pass
class Cursor:
    def __init__(s, buf, p, end): s.b=buf; s.p=p; s.end=end
    def left(s): return s.p < s.end

def read_vle(c):
    v=0
    while c.p < c.end:
        ch=c.b[c.p]; c.p+=1
        if ch & 0x80: v = v*128 + (ch-0x7f)
        else: return v + ch
    return v

def read_u32(c):
    if c.end - c.p < 4: c.p = c.end; return 0
    v=struct.unpack_from('<I', c.b, c.p)[0]; c.p+=4; return v

def std_decode_u32(code, count, cur, name):
    # returns list; FAIL per device (NLStandardDecoder::Decode<u32>)
    vals=[]
    end=cur.end
    if code==0x11:
        while cur.p<end and len(vals)<count: vals.append(read_u32(cur))
        if len(vals)!=count or cur.p!=end: raise Fail(f"{name}: raw decoded {len(vals)}/{count} p={cur.p:x}/{end:x}")
    elif code==0x12:
        nxt=0
        while cur.p<end:
            n=read_u32(cur)
            while nxt<n and len(vals)<count: vals.append(nxt); nxt+=1
        while len(vals)<count: vals.append(nxt); nxt+=1
        if cur.p!=end: raise Fail(f"{name}: raw-RLE p={cur.p:x}!={end:x}")
    elif code==0x13:
        pos=0; n=0
        while cur.p<end and n<count:
            ch=cur.b[cur.p]; cur.p+=1
            for j in range(8):
                if (ch>>j)&1:
                    vals.append(pos)
                    n+=1
                    if n==count: break
                pos+=1
        if n!=count: raise Fail(f"{name}: bitmap decoded {n}/{count}")
    elif code==0x14:
        while cur.p<end and len(vals)<count: vals.append(read_vle(cur))
        if len(vals)!=count or cur.p!=end: raise Fail(f"{name}: VLE decoded {len(vals)}/{count} p={cur.p:x}/{end:x}")
    elif code==0x15:
        nxt=0
        while cur.p<end:
            n=read_vle(cur)
            while nxt<n and len(vals)<count: vals.append(nxt); nxt+=1
        while len(vals)<count: vals.append(nxt); nxt+=1
        if cur.p!=end: raise Fail(f"{name}: VLE-RLE p={cur.p:x}!={end:x}")
    elif code==0x16:
        acc=0
        while cur.p<end and len(vals)<count: acc+=read_vle(cur); vals.append(acc)
        if len(vals)!=count or cur.p!=end: raise Fail(f"{name}: dVLE decoded {len(vals)}/{count} p={cur.p:x}/{end:x}")
    elif code==0x17:
        acc=0; nxt=0
        while cur.p<end:
            v=read_vle(cur); acc+=v
            while nxt<acc and len(vals)<count: vals.append(nxt); nxt+=1
        while len(vals)<count: vals.append(nxt); nxt+=1
    elif code==0x18:
        while cur.p<end:
            w=read_u32(cur)
            m=w>>28
            if m==0: continue
            if m not in S9: raise Fail(f"{name}: S9 bad tag {m}")
            cnt,nb=S9[m]; mask=(1<<nb)-1; x=w
            for i in range(cnt):
                if len(vals)>=count: break
                vals.append(x&mask); x>>=nb
        if len(vals)!=count: raise Fail(f"{name}: S9 decoded {len(vals)} != {count}")
    else:
        raise Fail(f"{name}: unsupported std code {code:#x}")
    return vals

def bitfield_decode(code, count, cur, name):
    # returns (bits list, ok)
    bits=[False]*count
    end=cur.end
    if code==0x01:
        while cur.p<end:
            for j in range(8):
                if len([b for b in bits if b])>=0 and bits.count if False else True: pass
            # positional fill LSB-first
            v=cur.b[cur.p]; cur.p+=1
            base=0
            # fill next up-to-8 bits
            idx=0
            # need running index: emulate device: uVar5 running
            # simpler: iterate positions
            bits=[b for b in bits]
            break
        # redo properly below
        cur.p=end
        bits=[False]*count
        p=0
        # device raw: per byte fills up to 8 consecutive bits
        curp2=struct.unpack_from  # noqa
        # emulate exactly:
        cc=Cursor(cur.b, cur.p0 if hasattr(cur,'p0') else cur.p, end)
        raise Fail("raw bitmap: handled by caller")
    return bits

def bitfield_decode_full(code, count, buf, off, ln, name):
    bits=[False]*count
    cur=Cursor(buf, off, off+ln)
    if code==0x01:
        p=off; end=off+ln; u=0
        while p<end and u<count:
            ch=buf[p]; p+=1
            for j in range(8):
                if u>=count: break
                bits[u]=bool((ch>>j)&1); u+=1
                if (u)==0: break
            # device: after each byte, bits filled = min(u+8, count)
        cur.p=p
    elif code in (0x02,0x03):
        init = (code==0x03)
        bits=[init]*count
        p=off; end=off+ln; pos=0; n=0
        while n<count and p<end:
            # VLE
            v=0
            while p<end:
                ch=buf[p]; p+=1
                if ch&0x80: v=v*128+(ch-0x7f)
                else: v+=ch; break
            pos+=v
            if 0<=pos<count: bits[pos]=not init
            n+=1
        cur.p=p
    else:
        raise Fail(f"{name}: bad bitfield code {code:#x}")
    return bits, cur.p

def decode_block(b, bs, be, opts6_12_off=True):
    nc,f2,nbel,nd = struct.unpack_from('<HHHH', b, bs)
    rows=[]
    for r in range(nd):
        q=bs+8+12*r
        k,code=struct.unpack_from('<HH',b,q); off,param=struct.unpack_from('<II',b,q+4)
        rows.append([k,code,off,param])
    # spans
    spans=[]
    for i,(k,code,off,param) in enumerate(rows):
        e = bs+rows[i+1][2] if i+1<nd else be
        spans.append(max(e-(bs+off),0))
    def find(k, flag):
        for i,(kk,code,off,param) in enumerate(rows):
            if kk==k: return (i)
        return None
    def row_of(kind_id, fbits):
        for i,(kk,code,off,param) in enumerate(rows):
            if (kk&0xfff)==kind_id and (kk&0xf000)==fbits: return i
        return None
    def desc(i):
        k,code,off,param = rows[i]
        return (bs+off, spans[i], param, code)

    def member(rows, id_, fb):
        i=row_of(id_,fb)
        return desc(i) if i is not None else (bs,0,0,0)

    log=[]
    def std(id_, fb, count, name):
        off,ln,cnt,code = member(rows, id_, fb)
        cur=Cursor(b, off, off+ln)
        r=std_decode_u32(code, count if count is not None else cnt, cur, name)
        return r
    def bf(id_, fb, name):
        off,ln,cnt,code = member(rows, id_, fb)
        bits,p = bitfield_decode_full(code, cnt, b, off, ln, name)
        return bits

    # ---- SetDescription + Decode order ----
    # 0401 (edge labels offsets+blob): offsets kind 0x8403 exact, blob 0x0403 exact
    ioff = next((i for i,r in enumerate(rows) if r[0]==0x8403), None)
    iblob = next((i for i,r in enumerate(rows) if r[0]==0x0403), None)
    if ioff is None: raise Fail("missing 0x8403 offsets row")
    off,ln,cnt,code = desc(ioff)
    std_decode_u32(code, cnt, Cursor(b,off,off+ln), "0403.offsets")
    log.append("0403 offsets ok")
    if iblob is not None:
        off,ln,cnt,code=desc(iblob)  # raw copy, code unchecked
    else:
        off,ln,cnt,code = bs,0,0,0
    log.append(f"0403 blob span={ln}")
    # 0402 block links: bitmap(0x4000) count nc; pairs(0x0000) count=2*popcount
    bits = bf(0x402, 0x4000, "0402.bitmap")
    if len(bits)!=nc: raise Fail("0402 bitmap count != nodes")
    npairs=sum(bits)*2
    off,ln,cnt,code = member(rows,0x402,0x0000)
    pairs = std_decode_u32(code, npairs, Cursor(b,off,off+ln), "0402.pairs")
    log.append(f"0402 links={sum(bits)}")
    # 0406 SimpleList<u32>
    off,ln,cnt,code = member(rows,0x406,0x0000)
    std_decode_u32(code, cnt, Cursor(b,off,off+ln), "0406")
    log.append("0406 ok")
    # 0401 SimpleList<u16>
    off,ln,cnt,code = member(rows,0x401,0x0000)
    odv = std_decode_u32(code, cnt, Cursor(b,off,off+ln), "0401")  # u16 table = same codes
    odv = (odv + [0]*nc)[:nc]
    log.append("0401 ok")
    # block header field +2 = forest root count = nc - sum(outdeg)
    roots = nc - sum(odv)
    if f2 != roots: raise Fail(f"header+2={f2} != nc-sum(od)={roots} (forest roots)")
    log.append(f"roots={roots}")
    # 0404/0405 ValueList<u16/u32>: bitmap(0x4000), offsets(0x8000) std u32, values(0x0000) VLD
    for id_ in (0x404,0x405):
        bmt = bf(id_,0x4000,f"{id_:04x}.bitmap")
        off,ln,cnt,code = member(rows,id_,0x8000)
        offs = std_decode_u32(code, cnt, Cursor(b,off,off+ln), f"{id_:04x}.offsets")
        off,ln,cnt,code = member(rows,id_,0x0000)
        # ValueListDecoder table {0x11,0x12,0x13,0x14,0x15,0x16,0x17,0x18}
        if cnt==0:
            std_decode_u32(code,0,Cursor(b,off,off+ln),f"{id_:04x}.values") if code!=0 else None
        else:
            std_decode_u32(code, cnt, Cursor(b,off,off+ln), f"{id_:04x}.values")
        log.append(f"{id_:04x} ok")
    # 0413 Binary (unconditional)
    off,ln,cnt,code = member(rows,0x413,0x0000)
    bitfield_decode_full(code,cnt,b,off,ln,"0413")
    log.append("0413 ok")
    # 0407 Position (opts0): bitmap(0x4000) count, values(0x0000) = 2*popcount
    pm = bf(0x407,0x4000,"0407.bitmap")
    off,ln,cnt,code = member(rows,0x407,0x0000)
    std_decode_u32(code, 2*sum(pm), Cursor(b,off,off+ln), "0407.values")
    log.append(f"0407 ok pop={sum(pm)}")
    # SingleValue kinds: 0408,0409,040c,040b,040a (opts1..5 ON in stock? stock has bitmap rows -> decode)
    for id_ in (0x408,0x409,0x40c,0x40b,0x40a):
        bm = bf(id_,0x4000,f"{id_:04x}.bitmap") if row_of(id_,0x4000) is not None else None
        if bm is None:
            # single 0x0000 row only -> bitmap member zero -> bitfield code0 -> FAIL per device
            raise Fail(f"{id_:04x}: no bitmap row but opts decode path hits code0")
        off,ln,cnt,code = member(rows,id_,0x0000)
        pop=sum(bm)
        if pop: std_decode_u32(code,pop,Cursor(b,off,off+ln),f"{id_:04x}.values")
        else:
            cur=Cursor(b,off,off+ln)
            if code not in (0x11,0x14,0x16,0x18):
                pass  # device still calls std -> would FAIL; but stock 0x409 c11 cnt0: loop none, 0==0 ok, code11 lands check p==end (span0 ok)
            std_decode_u32(code,0,Cursor(b,off,off+ln),f"{id_:04x}.values")
        log.append(f"{id_:04x} ok pop={pop}")
    # 0415 BinList: values(0x8000) count this+8; NON-EMPTY required: enGetEntryCharacterStatus
    # returns 4 (element dropped from every list/property query) when the values member is empty.
    off,ln,cnt,code = member(rows,0x415,0x8000)
    if cnt==0: raise Fail("0415 values member empty -> enGetEntryCharacterStatus=4 -> element invisible")
    std_decode_u32(code,cnt,Cursor(b,off,off+ln),"0415.values")
    off,ln,cnt,code = member(rows,0x415,0x0000)
    bitfield_decode_full(code,cnt,b,off,ln,"0415.bitmap")
    log.append("0415 ok")
    # 040e SingleValue (any opts)
    if row_of(0x40e,0x4000) is not None:
        bm=bf(0x40e,0x4000,"040e.bitmap"); pop=sum(bm)
        off,ln,cnt,code=member(rows,0x40e,0x0000)
        if pop: std_decode_u32(code,pop,Cursor(b,off,off+ln),"040e.values")
        else: std_decode_u32(code,0,Cursor(b,off,off+ln),"040e.values")
        log.append(f"040e ok pop={pop}")
    # Binary kinds (count==0 -> return1 skip; else bitfield): 040d,040f,0410,0411,0412,0414,0416
    pops={}
    for id_ in (0x40d,0x40f,0x410,0x411,0x412,0x414,0x416):
        off,ln,cnt,code = member(rows,id_,0x0000)
        if cnt!=0:
            bl,_p=bitfield_decode_full(code,cnt,b,off,ln,f"{id_:04x}")
            pops[id_]=sum(bl)
        else:
            pops[id_]=0
        log.append(f"{id_:04x} ok pop={pops[id_]}")
    # UI gate: bIsEntryValidDestination (040d member +0x35c) - empty vector/false bit = element
    # reported as non-destination (list processors skip it in destination browsing).
    if pops.get(0x40d,0)==0: print(f"  WARN: 040d all-false/absent -> elements report not-a-destination")
    # ---- device TEIC consistency (CalculateFirstEdgeIndex + ProcessSubTreeTEIC, pure DFS) ----
    cs=[0]*nc; ec=0; base=1; r=0; nterm=0
    while r<nc:
        stack=[r]; visits=0
        while stack:
            x=stack.pop()
            if x>=nc: raise Fail("child index beyond node count (bad tree)")
            cs[x]=base+ec
            ec+=odv[x]; visits+=1
            c0=cs[x]
            for i in range(odv[x]-1,-1,-1):
                c=c0+i
                if c<nc: stack.append(c)
        r+=visits; base+=1
    for x in range(nc):
        if cs[x]+odv[x]>nc: raise Fail(f"node{x}: children {cs[x]}+{odv[x]} beyond nc")
    # terminating elements: leaf && no block-link, in root-DFS order
    cs2=[0]*nc; ec=0; base=1; r=0
    while r<nc:
        stack=[r]; visits=0
        while stack:
            x=stack.pop()
            cs2[x]=base+ec
            ec+=odv[x]; visits+=1
            c0=cs2[x]
            for i in range(odv[x]-1,-1,-1):
                c=c0+i
                if c<nc: stack.append(c)
        r+=visits; base+=1
    # count terminals EXACTLY as the device caller CalculateTerminatingElementIndex:
    # root loop jumps by subtree-EDGE-COUNT+1 (DAWG sharing makes it skip shared regions).
    r=0; cnt=0; iters=0
    while r<nc and iters<10000:
        iters+=1
        stack=[r]; ecnt=0
        while stack:
            x=stack.pop()
            c0=cs2[x]
            ecnt+=odv[x]
            for i in range(odv[x]):
                c=c0+i
                if c<nc and odv[c]==0 and not bits[c]: cnt+=1
            for i in range(odv[x]-1,-1,-1):
                c=c0+i
                if c<nc and odv[c]!=0: stack.append(c)
        r+=ecnt+1
    if cnt!=nbel:
        if 0<=cnt-nbel<=12: print(f"  note: TEIC {cnt} vs nbel {nbel} (+drift, stock-typical up to +9; device TEIC vector oversized)")
        else: raise Fail(f"TEIC terminates {cnt} != nbel {nbel}")
    if iters!=f2: print(f"  note: TEIC root-iters {iters} vs header+2 {f2}")
    log.append(f"teic ok terms={cnt} roots={iters}")
    return log


def analyze_file(path, tag):
    """Full-file validation mirroring NLNameList::LoadHeader + NLProcessor::bInitialise."""
    b=open(path,'rb').read()
    hdroff=struct.unpack_from('<I',b,0x10)[0]
    hdrsize=struct.unpack_from('<I',b,0x14)[0]
    # region1: file@0: magic,listID,u16,u16,u32,u32,hdr,size (LoadHeader reads region1 via this+0x60..)
    elem=struct.unpack_from('<I',b,hdroff)[0]
    nb=struct.unpack_from('<H',b,hdroff+4)[0]
    nrec=struct.unpack_from('<H',b,hdroff+6)[0]
    c4=struct.unpack_from('<H',b,hdroff+8)[0]
    c5=struct.unpack_from('<H',b,hdroff+10)[0]
    c6=struct.unpack_from('<H',b,hdroff+12)[0]
    p=hdroff+0x18
    secs=[(b[p+5*i], struct.unpack_from('<I',b,p+5*i+1)[0]) for i in range(7)]
    # sec6 must end exactly at hdroff+hdrsize (LoadHeader cursor rule for last section)
    counts=[nb,nb,nrec,nrec,c4,c5,c6]
    is16=[False,False,False,False,True,True,True]
    ends=[secs[1][1],secs[2][1],secs[3][1],secs[4][1],secs[5][1],secs[6][1],hdroff+hdrsize]
    for i,(code,off) in enumerate(secs):
        ln=ends[i]-off
        if ln<0: raise Fail(f"{tag} sec{i}: negative length")
        cur=Cursor(b,off,off+ln)
        cnt=std_decode_u32(code, counts[i], cur, f"{tag}.sec{i}")
        if len(cnt)!=counts[i]: raise Fail(f"{tag} sec{i}: decoded {len(cnt)} != count {counts[i]}")
        if cur.p!=ends[i]: raise Fail(f"{tag} sec{i}: cursor {cur.p} != end {ends[i]}")
    offs=[struct.unpack_from('<I',b,secs[1][1]+4*i)[0] for i in range(nb)]
    sizes=[struct.unpack_from('<I',b,secs[0][1]+4*i)[0] for i in range(nb)] if False else None
    # sizes stream: VLE u32 x nb
    cur=Cursor(b, secs[0][1], secs[1][1])
    sz=std_decode_u32(secs[0][0], nb, cur, f"{tag}.sizes")
    if len(sz)!=nb: raise Fail("sizes count")
    for i in range(nb):
        exp=(offs[i+1] if i+1<nb else len(b))
        if sz[i]>exp-offs[i]: print(f"{tag} WARN: block{i}: sec0 length-hint {sz[i]} exceeds span {exp-offs[i]}")
    ok=0
    for bi in range(nb):
        bs=offs[bi]; be=offs[bi+1] if bi+1<nb else len(b)
        try:
            decode_block(b,bs,be); ok+=1
        except Fail as e:
            print(f"{tag} block{bi}: *** LOAD FAIL: {e}")
    # relation records (sec2/sec3): header field hdroff+8 = relations this list participates in;
    # stock invariant nrec == nb * nrel (35-byte record per (block,relation)). A file promising
    # nrel>0 relations with nrec==0 was the top suspect for the 2026-09-25 10-city card reset.
    nrel=struct.unpack_from('<H',b,hdroff+8)[0]
    if nrel>0 and nrec==0:
        print(f"{tag} *** GATE FAIL: nrel={nrel} but nrec=0 (unfulfilled relation promise)")
    if nrec>0 and nrel>0:
        if nrec!=nb*nrel: print(f"{tag} *** GATE FAIL: nrec={nrec} != nb*nrel={nb*nrel}")
        roffs=[struct.unpack_from('<I',b,secs[3][1]+4*i)[0] for i in range(nrec)]
        for i,ro in enumerate(roffs):
            if ro+35>len(b): raise Fail(f"{tag} record{i} out of file")
            if struct.unpack_from('<I',b,ro)[0]!=35: raise Fail(f"{tag} record{i} size != 35")
    print(f"{tag}: header OK (elem={elem} nb={nb} nrec={nrec} nrel={nrel} langs={c5} cats={c6}); {ok}/{nb} blocks decode OK")

def analyze(path, bis, tag):
    b=open(path,'rb').read()
    hdr=struct.unpack_from('<I',b,0x10)[0]
    nb=struct.unpack_from('<H',b,hdr+4)[0]
    p=hdr+0x18
    secs=[(b[p+5*i], struct.unpack_from('<I',b,p+5*i+1)[0]) for i in range(7)]
    offs=[struct.unpack_from('<I',b,secs[1][1]+4*i)[0] for i in range(nb)]
    for bi in bis:
        bs=offs[bi]; be=offs[bi+1] if bi+1<nb else len(b)
        try:
            log=decode_block(b,bs,be)
            print(f"{tag} block{bi}: LOAD OK ({'; '.join(log)})")
        except Fail as e:
            print(f"{tag} block{bi}: *** LOAD FAIL: {e}")

if __name__=='__main__':
    B='/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL'
    analyze_file(B+'/LID20001.DAT','STOCK')
    analyze_file('/tmp/opencode/zzmin1.DAT','MIN')
    analyze('/tmp/opencode/zz20001h.DAT', [44,45,46], 'V11')
