import struct, sys
def dump(path, bi, tag):
    b=open(path,'rb').read()
    hdr=struct.unpack_from('<I',b,0x10)[0]
    nb=struct.unpack_from('<H',b,hdr+4)[0]
    boff=struct.unpack_from('<I',b,hdr+30)[0]
    bo=[struct.unpack_from('<I',b,boff+4*i)[0] for i in range(nb)]
    bs=bo[bi]; be=bo[bi+1] if bi+1<nb else len(b)
    u16=lambda p: struct.unpack_from('<H',b,p)[0]
    u32=lambda p: struct.unpack_from('<I',b,p)[0]
    nc=u16(bs); nred=u16(bs+2); nbel=u16(bs+4); nd=u16(bs+6)
    print(f"== {tag} block{bi}: hdr nc={nc} @+2={nred} nbel={nbel} ndesc={nd}")
    # descriptor rows for 0x402
    rows=[]
    for r in range(nd):
        p=bs+8+r*12; k=u16(p)
        rows.append((k&0xfff,k&0xf000,u16(p+2),u32(p+4),u32(p+8)))
    def se(i): return max(bs+rows[i+1][3],bs+rows[i][3]) if i+1<len(rows) else be
    for i,(k,fl,code,off,param) in enumerate(rows):
        if k!=0x402: continue
        s=bs+off; e=se(i); span=e-s if e>s else 0
        print(f"  row{i} flags={fl:05x} code={code:02x} off={off:#x} span={span} param={param} bytes={b[s:s+min(span,40)].hex()}")
    # what is between block header (8+12*nd) and first row? and tail after last row: scan for pair-looking arrays
    hdr_end=bs+8+nd*12
    first_off=min(r[3] for r in rows)
    print(f"  gap hdr_end={hdr_end-bs:#x} first_row_off={first_off:#x}")
    gap=b[bs+8+nd*12:bs+first_off]
    print(f"  gap bytes ({len(gap)}): {gap[:120].hex()}")
    print(f"  tail bytes after size? size field candidates: bytes @bs+8..24: {b[bs+8:bs+24].hex()}")
B='/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL'
dump(B+'/LID20001.DAT',44,'STOCK')
dump('zz20001h.DAT',44,'V11')
