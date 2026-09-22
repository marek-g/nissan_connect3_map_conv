use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let needles: Vec<u32> = a[1..].iter().map(|x| x.parse().unwrap()).collect();
    let ga = read_gen_attr(&buf).unwrap();
    for bi in 0..ga.blocks.len() {
        let blk = match ga.decode_block(&buf, bi) { Ok(b)=>b, Err(_)=>continue };
        // every range-attr col (flags 0x8000 starts + values) in this block: print values at descr ids 1885..1899
        for col in [0x0c08u32,0x0c10,0x0c14,0x0c15,0x0c16,0x0c0d,0x0c0e,0x0c0f] {
            let s8 = blk.streams.iter().find(|x| x.col==col&&x.flags==0x8000).map(|s|s.values.clone());
            let v0 = blk.streams.iter().find(|x| x.col==col&&x.flags==0).map(|s|s.values.clone()).unwrap_or_default();
            if let Some(s8)=s8 {
                // starts vector indexed by domain ordinal; assume ord==index
                let mut out = vec![];
                for d in 1885..=1899u32 {
                    let i=d as usize;
                    if i+1<s8.len() { let (st,en)=(s8[i] as usize,s8[i+1] as usize); if st<en && en<=v0.len() { out.push((d, &v0[st..en])); } }
                }
                if !out.is_empty() { println!("block {bi} col {col:#x}: {out:?}"); }
            }
        }
        // global needle scan per col values:
        for s in &blk.streams { if s.flags==0 && needles.iter().any(|n| s.values.contains(n)) { println!("block {bi} col {:x} flags {:x} CONTAINS needle", s.col, s.flags); } }
    }
}
