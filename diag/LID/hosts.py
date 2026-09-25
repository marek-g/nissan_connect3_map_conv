import struct
B='/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL'
b=open(B+'/LID20001.DAT','rb').read()
hdr=struct.unpack_from('<I',b,0x10)[0]
nb=struct.unpack_from('<H',b,hdr+4)[0]
p=hdr+0x18
secs=[(b[p+5*i], struct.unpack_from('<I',b,p+5*i+1)[0]) for i in range(7)]
offs=[struct.unpack_from('<I',b,secs[1][1]+4*i)[0] for i in range(nb)]
def rvle(b,p,lim):
    v=0
    while p<lim:
        c=b[p];p+=1
        if c&0x80: v=v*128+(c-0x7F)
        else: return v+c,p
    return v,p
M={1:(28,1),2:(14,2),3:(9,3),4:(7,4),5:(5,5),6:(4,7),7:(3,9),8:(2,14),9:(1,28)}
def outdeg_and_bitfields(bi):
    bs=offs[bi]; nc=struct.unpack_from('<H',b,bs)[0]
    nd=struct.unpack_from('<H',b,bs+6)[0]
    descs=[]
    for r in range(nd):
        q=bs+8+12*r; k=struct.unpack_from('<H',b,q)[0]
        descs.append((k&0xfff,k&0xf000,struct.unpack_from('<H',b,q+2)[0],struct.unpack_from('<I',b,q+4)[0],struct.unpack_from('<I',b,q+8)[0]))
    def se(r): return (bs+descs[r+1][3]) if r+1<nd else (offs[bi+1] if bi+1<nb else len(b))
    ri=[i for i,d in enumerate(descs) if d[0]==0x401 and d[1]==0][0]
    code=descs[ri][2]; off=bs+descs[ri][3]; cnt=descs[ri][4]; e=se(ri); p2=off; od=[]
    if code==0x18:
        while p2+4<=e:
            w=struct.unpack_from('<I',b,p2)[0];p2+=4;m=w>>28
            if m==0:continue
            c9,bits=M[m];mask=(1<<bits)-1
            for kk in range(c9):
                if len(od)>=cnt:break
                od.append((w>>(kk*bits))&mask)
    elif code==0x14:
        while p2<e and len(od)<cnt: v,p2=rvle(b,p2,e);od.append(v)
    od+= [0]*(nc-len(od))
    ri0=[i for i,d in enumerate(descs) if d[0]==0x402 and d[1]==0x4000][0]
    d0=descs[ri0]; e0=se(ri0); p3=bs+d0[3]; pos=0; hosts=[]
    while p3<e0:
        v,p3=rvle(b,p3,e0); pos+=v; hosts.append(pos)
    return nc,od,hosts
for bi in [40,41,42,43]:
    nc,od,hosts=outdeg_and_bitfields(bi)
    print(f"blk{bi} nc={nc}: {len(hosts)} hosts={hosts}")
    print(f"   outdeg[host]={ [od[h] for h in hosts] }  terminal(outdeg==0)={[h for h in hosts if od[h]==0]}")
