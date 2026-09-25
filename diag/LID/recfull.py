import struct
B='/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL'
def recs(path,label,blks):
    b=open(path,'rb').read()
    hdr=struct.unpack_from('<I',b,0x10)[0]
    nb=struct.unpack_from('<H',b,hdr+4)[0]
    p=hdr+0x18
    secs=[(b[p+5*i], struct.unpack_from('<I',b,p+5*i+1)[0]) for i in range(7)]
    s3o=secs[3][1]
    s3=[struct.unpack_from('<I',b,s3o+4*i)[0] for i in range(4*nb)]
    rec=35
    print(f"== {label} nb={nb}")
    for bi in blks:
        if 4*bi>=len(s3): print(f"  blk{bi}: NO RECORDS"); continue
        for j in range(4):
            r=b[s3[4*bi+j]:s3[4*bi+j]+rec]
            print(f"  blk{bi} rec{j}: {' '.join(f'{x:02x}' for x in r)}")
recs(B+'/LID20001.DAT','STOCK',[0,1,2,43,44])
recs('zz20001h.DAT','V11',[0,44,45,46])
