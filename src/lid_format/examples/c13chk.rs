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
                for s in &blk.streams {
                    if s.flags == 0x4000
                        && s.bits.len() == (blk.elem_end - blk.elem_start + 1) as usize
                    {
                        println!("blk {bi} sid {sid} col {:x} bit {}", s.col, s.bits[ord]);
                    }
                }
                if let Some(b13) = blk
                    .streams
                    .iter()
                    .find(|x| x.col == 0xc13 && x.flags == 0x4000)
                {
                    println!(
                        "  c13 popcount {} width {}",
                        b13.bits.iter().filter(|&&x| x).count(),
                        b13.bits.len()
                    );
                }
            }
        }
    }
}
