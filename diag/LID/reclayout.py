import struct
B='/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL'
def analyze(path,label):
    b=open(path,'rb').read()
    hdr=struct.unpack_from('<I',b,0x10)[0]
    nb=struct.unpack_from('<H',b,hdr+4)[0]
    p=hdr+0x18
    secs=[(b[p+5*i], struct.unpack_from('<I',b,p+5*i+1)[0]) for i in range(7)]
    offs=[struct.unpack_from('<I',b,secs[1][1]+4*i)[0] for i in range(nb)]
    def rvle(pp,lim):
        v=0
        while pp<lim:
            c=b[pp];pp+=1
            if c&0x80: v=v*128+(c-0x7F)
            else: return v+c,pp
        return v,pp
    # sec2: per-record VLE (4a values); sec3: record offsets raw u32
    s2c,s2o=secs[2]; s3o=secs[3][1]
    p2=s2o; rsizes=[]
    for i in range(4*nb):
        v,p2=rvle(p2,s3o); rsizes.append(v)
    rec=rsizes[0]
    s3=[struct.unpack_from('<I',b,s3o+4*i)[0] for i in range(4*nb)]
    nbel=[struct.unpack_from('<H',b,offs[i]+4)[0] for i in range(nb)]
    nc=[struct.unpack_from('<H',b,offs[i])[0] for i in range(nb)]
    ne=[struct.unpack_from('<H',b,offs[i]+2)[0] for i in range(nb)]
    print(f"== {label}: rec size={rec} nb={nb}")
    # cumulative nbel bases
    base=[];acc=0
    for i in range(nb):
        base.append(acc);acc+=nbel[i]
    print(f"  Σnbel={acc}")
    tot=struct.unpack_from('<I',b,hdr)[0]
    print(f"  elem_count hdr={tot}")
    # for first few blocks and last few: dump 4 records fields vs (base, nbel)
    for bi in ([0,1,43,44] if nb>44 else [0,1,nb-1]):
        recs=[b[s3[4*bi+j]:s3[4*bi+j]+rec] for j in range(4)]
        u=lambda r,o: struct.unpack_from('<I',r,o)[0]
        print(f"  blk{bi} base={base[bi]} nbel={nbel[bi]} nc={nc[bi]} @+2={ne[bi]}")
        for j,r in enumerate(recs):
            fs=[o for o in range(0,min(rec,20),4)]
            print(f"    rec{j}: "+" ".join(f"[{o}]={u(r,o):#x}({u(r,o)})" for o in fs))
    # check hypothesis record[4]=global elem base for all blocks
    ok=all(u:=struct.unpack_from('<I',b,s3[4*i]+4)[0]==base[i] for i in range(nb))
    print(f"  record[+4]==Σnbel_before for ALL blocks: {ok}")
    ok2=all(struct.unpack_from('<I',b,s3[4*i]+4)[0]+nbel[i]<=tot for i in range(nb))
    print(f"  record[+4]+nbel <= elem_total all: {ok2}")
analyze(B+'/LID20001.DAT','STOCK')
analyze('zz20001h.DAT','V11')
