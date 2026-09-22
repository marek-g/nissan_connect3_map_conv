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
            for s in &blk.streams { if s.col==1 { println!("col1 fl {:x}: bits {} vals {}", s.flags, s.bits.iter().take(4).count(), s.values.len()); if s.flags==0x4000 { println!("  ord {ord} bit {}", s.bits[ord]); } } }
            let s1=blk.streams.iter().find(|x|x.col==1&&x.flags==0x8000).map(|s|s.values.clone()).unwrap_or_default();
            let v1=blk.streams.iter().find(|x|x.col==1&&x.flags==0).map(|s|s.values.clone()).unwrap_or_default();
            let ob=blk.streams.iter().find(|x|x.col==1&&x.flags==0x4000).map(|s|s.bits.clone()).unwrap();
            let po=ob[..=ord].iter().filter(|&&x|x).count().saturating_sub(1);
            println!("starts {} vals {} po {po} s1[po]={} s1[po+1]={}", s1.len(), v1.len(), s1.get(po).copied().unwrap_or(9999), s1.get(po+1).copied().unwrap_or(9999));
        }
    }
}
