use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let sids: Vec<u32> = a[1..].iter().map(|x| x.parse().unwrap()).collect();
    let ga = read_gen_attr(&buf).unwrap();
    for bi in 0..ga.blocks.len() {
        for &sid in &sids {
            if ga.blocks[bi].elem_start <= sid && sid <= ga.blocks[bi].elem_end {
                let blk = ga.decode_block(&buf, bi).unwrap();
                let ord = (sid - ga.blocks[bi].elem_start) as usize;
                let g = |c: u32, f: u32| blk.streams.iter().find(|x| x.col == c && x.flags == f);
                println!(
                    "sid {sid} blk {bi}: col1 rows {:?}",
                    blk.streams
                        .iter()
                        .filter(|s| s.col == 1)
                        .map(|s| (s.flags, s.bits.len(), s.values.len()))
                        .collect::<Vec<_>>()
                );
                if let (Some(s1), Some(v1)) = (g(1, 0x8000), g(1, 0)) {
                    let s1 = &s1.values;
                    let v1 = &v1.values;
                    let po = match g(1, 0x4000) {
                        Some(b) => b.bits[..=ord]
                            .iter()
                            .filter(|&&x| x)
                            .count()
                            .saturating_sub(1),
                        None => ord,
                    };
                    if po < s1.len() {
                        let st = s1[po] as usize;
                        let en = s1.get(po + 1).map(|&x| x as usize).unwrap_or(v1.len());
                        println!("  owners {:?}", &v1[st..en]);
                    } else {
                        println!("  po {po} >= starts {}", s1.len());
                    }
                }
                println!(
                    "  c01 exists {}",
                    g(0xc01, 0x4000).map_or(false, |b| b.bits[ord])
                );
            }
        }
    }
}
