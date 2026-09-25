import struct
def analyze(path, tag):
    b=open(path,'rb').read()
    hdr=struct.unpack_from('<I',b,0x10)[0]
    nb=struct.unpack_from('<H',b,hdr+4)[0]
    boff=struct.unpack_from('<I',b,hdr+30)[0]
    bo=[struct.unpack_from('<I',b,boff+4*i)[0] for i in range(nb)]
    sec3=struct.unpack_from('<I',b,hdr+24+3*5+1)[0]
    nrec=struct.unpack_from('<H',b,hdr+6)[0]
    tail0=min(struct.unpack_from('<I',b,sec3+4*i)[0] for i in range(nrec))
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
    def s9(p,lim):
        n=0; land=True
        while p+4<=lim:
            w=u32(p); p+=4
            m=w>>28
            if m==0: continue
            if m not in MODES: return (-1,False)
            n+=MODES[m]
        if p!=lim: land=False
        return (n,land)
    def vle(p,lim):
        n=0
        while p<lim:
            v,p=readvle(p,lim); n+=1
        return n
    bs=bo[44]; be=bo[45] if 45<nb else tail0
    nd=u16(bs+6); nbel=u16(bs+4)
    rows=[]
    for r in range(nd):
        p=bs+8+r*12; k=u16(p)
        rows.append((k&0xfff,k&0xf000,u16(p+2),u32(p+4),u32(p+8)))
    def se(i): return max(bs+rows[i+1][3],bs+rows[i][3]) if i+1<len(rows) else be
    print(f"== {tag}: blk44 nbel={nbel} size={be-bs}")
    for i,(k,fl,code,off,param) in enumerate(rows):
        s=bs+off; e=se(i)
        span=e-s if e>s else 0
        if code in (1,2,3):
            if code==1:
                need=(param+7)//8
                ok=(span==0 and param==0) or span>=need
                print(f"  row{i:2d} {k:04x}/{fl:05x} c{code:02x} param={param} span={span} RAW {'OK' if ok else 'FAIL need %d'%need}")
            else:
                p=s; np=0
                while p<e:
                    v,p=readvle(p,e); np+=1
                print(f"  row{i:2d} {k:04x}/{fl:05x} c{code:02x} param={param} span={span} DELTA n={np} {'OK' if p==e else 'FAIL'}")
        elif span==0:
            print(f"  row{i:2d} {k:04x}/{fl:05x} c{code:02x} param={param} span=0 EMPTY")
        elif code==0x14:
            n=vle(s,e)
            rules=[r for r,test in [("param",n==param),("2p",n==2*param),("4p",n==4*param)] if test]
            print(f"  row{i:2d} {k:04x}/{fl:05x} c14 param={param} span={span} VLE n={n} rules={rules}")
        elif code==0x18:
            n,land=s9(s,e)
            print(f"  row{i:2d} {k:04x}/{fl:05x} c18 param={param} span={span} S9 n={n} land={'OK' if land else 'FAIL'}")
        elif code==0x11:
            print(f"  row{i:2d} {k:04x}/{fl:05x} c11 param={param} span={span} RAW4 {'OK' if span==4*param or param==0 and span==0 else 'CHECK'}")
        else:
            print(f"  row{i:2d} {k:04x}/{fl:05x} c{code:02x} param={param} span={span} OTHER")
analyze('/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20001.DAT','STOCK')
analyze('zz20001h.DAT','V11')

