use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let needles: Vec<u32> = a[2..].iter().map(|x| x.parse().unwrap()).collect();
    let bi: usize = a[1].parse().unwrap();
    let ga = read_gen_attr(&buf).unwrap();
    let blk = ga.decode_block(&buf, bi).unwrap();
    println!("block {bi} elems {}..{} streams:", blk.elem_start, blk.elem_end);
    for s in &blk.streams { println!("  col {:x} flags {:x} bits {} vals {}", s.col, s.flags, s.bits.len(), s.values.len()); }
    for s in &blk.streams {
        if s.col==1 { for n in &needles { if let Some(p)=s.values.iter().position(|&x|x==*n){ println!("NEEDLE {n} col {:x} fl {:x} at idx {p}", s.col, s.flags);} } }
    }
}
