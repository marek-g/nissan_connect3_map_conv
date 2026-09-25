import struct, sys
def analyze(path, bi):
    b=open(path,'rb').read()
    hdr=struct.unpack_from('<I',b,0x10)[0]
    nb=struct.unpack_from('<H',b,hdr+4)[0]
    boff=struct.unpack_from('<I',b,hdr+30)[0]
    bo=[struct.unpack_from('<I',b,boff+4*i)[0] for i in range(nb)]
    def u16(p): return struct.unpack_from('<H',b,p)[0]
    def u32(p): return struct.unpack_from('<I',b,p)[0]
    def readvle(p,lim):
        v=0
        while p<lim:
            c=b[p];p+=1
            v=v*128+(c-0x7F if c&0x80 else c)
            if not c&0x80: break
        return v,p
    MODES={1:28,2:14,3:9,4:7,5:6,6:5,7:4,8:3,9:1}
    bs=bo[bi]; be=bo[bi+1] if bi+1<nb else len(b)
    nc=u16(bs); nbel=u16(bs+4); nd=u16(bs+6)
    rows=[]
    for r in range(nd):
        p=bs+8+r*12; k=u16(p)
        rows.append((k&0xfff,k&0xf000,u16(p+2),u32(p+4),u32(p+8)))
    def se(i): return max(bs+rows[i+1][3],bs+rows[i][3]) if i+1<len(rows) else be
    print(f"== block {bi}: nodes={nc} edges={u16(bs+2)} nbel={nbel} ndesc={nd} size={be-bs}")
    for i,(k,fl,code,off,param) in enumerate(rows):
        s=bs+off; e=se(i); span=e-s if e>s else 0
        tag=f"  row{i:2d} {k:04x}/{fl:05x} c{code:02x} param={param} span={span}"
        if code in (1,2,3):
            if code==1: print(tag,"RAW","OK" if (span==(param+7)//8 or (param==0 and span==0)) else "FAIL")
            else:
                p=s; np=0
                while p<e: v,p=readvle(p,e); np+=1
                print(tag,"DELTA","OK" if p==e else f"FAIL p={p:x}!={e:x}")
        elif span==0: print(tag,"EMPTY")
        elif code==0x14:
            p=s; n=0
            while p<e: v,p=readvle(p,e); n+=1
            print(tag,"VLE","n=",n,"","OK(param)" if n==param else ("OK(2p)" if n==2*param else "CHECK"))
        elif code==0x18:
            p=s; n=0; land=True
            while p+4<=e:
                w=u32(p); p+=4; m=w>>28
                if m==0: continue
                if m not in MODES: land=False; break
                n+=MODES[m]
            print(tag,"S9","n=",n,"","OK" if (p==e and land) else f"FAIL(land={land} p={p:x}/{e:x})")
        elif code==0x11: print(tag,"RAW","OK" if span in (param,4*param,0) else f"CHECK span%4={span%4} param={param}")
        elif code==0x16:
            p=s; n=0
            while p<e: v,p=readvle(p,e); n+=1
            print(tag,"DELTA8","n=",n,"OK" if p==e else "FAIL")
        else: print(tag,"OTHER")
analyze(sys.argv[1], int(sys.argv[2]))
