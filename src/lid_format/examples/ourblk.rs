use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let lo: u32 = a[1].parse().unwrap(); let hi: u32 = a[2].parse().unwrap();
    let ga = read_gen_attr(&buf).unwrap();
    println!("blocks {}", ga.blocks.len());
    for bi in 0..ga.blocks.len() {
        let t = &ga.blocks[bi];
        if t.elem_end >= lo && t.elem_start <= hi {
            let blk = ga.decode_block(&buf, bi).unwrap();
            println!("block {bi} elems {}..{}", t.elem_start, t.elem_end);
            for s in &blk.streams { println!("  col {:x} flags {:x} bits {} vals {}", s.col, s.flags, s.bits.len(), s.values.len()); }
        }
    }
}
