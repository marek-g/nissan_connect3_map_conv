use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let sid: u32 = a[1].parse().unwrap();
    let ga = read_gen_attr(&buf).unwrap();
    for bi in 0..ga.blocks.len() {
        if ga.blocks[bi].elem_start<=sid && sid<=ga.blocks[bi].elem_end {
            let blk = ga.decode_block(&buf,bi).unwrap();
            let ord=(sid-ga.blocks[bi].elem_start) as usize;
            let bits=blk.streams.iter().find(|x|x.col==0xc01&&x.flags==0x4000).map(|s|s.bits.clone()).unwrap();
            if !bits[ord] { println!("sid {sid}: no records"); return }
            let p=b1(&bits,ord);
            let s1=blk.streams.iter().find(|x|x.col==1&&x.flags==0x8000).map(|s|s.values.clone()).unwrap_or_default();
            let v1=blk.streams.iter().find(|x|x.col==1&&x.flags==0).map(|s|s.values.clone()).unwrap_or_default();
            let ob=blk.streams.iter().find(|x|x.col==1&&x.flags==0x4000).map(|s|s.bits.clone()).unwrap_or_default();
            let po = if !ob.is_empty() { b1(&ob,ord) } else { usize::MAX };
            let owners = if po!=usize::MAX && po+1<s1.len() { v1[s1[po] as usize..s1[po+1] as usize].to_vec() } else { vec![] };
            let s0=blk.streams.iter().find(|x|x.col==0xc01&&x.flags==0x8000).map(|s|s.values.clone()).unwrap();
            let v0=blk.streams.iter().find(|x|x.col==0xc01&&x.flags==0).map(|s|s.values.clone()).unwrap();
            let c11=blk.streams.iter().find(|x|x.col==0xc11&&x.flags==0).map(|s|s.values.clone()).unwrap();
            let (st,en)=(s0[p] as usize, s0.get(p+1).map(|&x|x as usize).unwrap_or(v0.len()));
            println!("sid {sid} owners {owners:?} nums {:?} c11 {:?}", &v0[st..en], &c11[st..en]);
        }
    }
}
fn b1(bits:&[bool],ord:usize)->usize{ bits[..=ord].iter().filter(|&&x|x).count()-1 }
