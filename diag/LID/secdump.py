import struct, sys
def readvle(b,p,lim):
    v=0
    while p<lim:
        c=b[p];p+=1
        if c&0x80: v=v*128+(c-0x7F)
        else: return v+c,p
    return v,p
MODES={1:(28,1),2:(14,2),3:(9,3),4:(7,4),5:(5,5),6:(4,7),7:(3,9),8:(2,14),9:(1,28)}
def dec(code,b,s,e,n,w):
    vals=[];p=s;acc=0
    if code in (0x11,0x12):
        while p+w<=e and len(vals)<n:
            vals.append(int.from_bytes(b[p:p+w],'little'));p+=w
        return vals,p
    if code in (0x13,):  # bitmap->positions (value list from bits): treat as bitfield-of-values
        # device u32: code 0x13: read bytes, set position i if bit set (values=positions)
        i=0
        while p<e and i<n:
            for k in range(8):
                if i>=n:break
                if b[p]>>k &1: vals.append(i);i+=1
                else: i+=1
            p+=1
        return vals,p
    if code in (0x14,0x15):
        while p<e and len(vals)<n:
            v,p=readvle(b,p,e);vals.append(v)
        return vals,p
    if code in (0x16,0x17):
        while p<e and len(vals)<n:
            v,p=readvle(b,p,e);acc+=v;vals.append(acc)
        return vals,p
    if code==0x18:
        while p+4<=e and len(vals)<n:
            x=struct.unpack_from('<I',b,p)[0];p+=4;m=x>>28
            if m==0:continue
            if m not in MODES:return vals,'BADMODE'
            cnt,bits=MODES[m];mask=(1<<bits)-1
            for k in range(cnt):
                if len(vals)>=n:break
                vals.append((x>>(k*bits))&mask)
        return vals,p
    return vals,'?'
def sec(path):
    b=open(path,'rb').read()
    base=struct.unpack_from('<I',b,0x10)[0]
    tl=struct.unpack_from('<I',b,0x14)[0]
    p=base
    v7c=struct.unpack_from('<I',b,p)[0];p+=4
    nb,vm84,vm88,vm8c,vm8e,vm90=struct.unpack_from('<6H',b,p);p+=12
    v94,v98=struct.unpack_from('<2I',b,p);p+=8
    codes=list(b[p:p+7]);p+=7
    offs=list(struct.unpack_from('<7I',b,p))
    print(f"== {path.split('/')[-1]}")
    print(f"  struct@{base:#x} len={tl:#x}  7c={v7c:#x} nb={nb} m84={vm84} m88={vm88} m8c={vm8c} m8e={vm8e} m90={vm90} 94={v94:#x} 98={v98:#x}")
    print(f"  codes={[hex(c) for c in codes]}")
    print(f"  offs={[hex(o) for o in offs]}")
    counts=[nb,nb,vm84,vm84,vm88,vm8c,vm8e]
    widths=[4,4,4,4,2,2,2]
    ends=offs[1:]+[base+tl]
    names=['OFFSETS','SIZES','m84a','m84b','m88','m8c','m8e']
    for i in range(7):
        vals,pend=dec(codes[i],b,offs[i],ends[i],counts[i],widths[i])
        land='OK' if pend==ends[i] else f'FAIL({pend:#x} vs {ends[i]:#x})'
        print(f"  vec{i} {names[i]:8s} code={codes[i]:#04x} cnt={counts[i]:6d} [{offs[i]:#x},{ends[i]:#x}) decoded={len(vals):6d} land={land}")
        if i<2:
            print(f"      entries 38..{min(len(vals),nb)}: {vals[38:nb+3]}")
B='/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL'
sec(B+'/LID20001.DAT')
for f in sys.argv[1:]: sec(f)
