use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let bi: usize = a[1].parse().unwrap();
    let needles: Vec<u32> = a[2..].iter().map(|x| x.parse().unwrap()).collect();
    let ga = read_gen_attr(&buf).unwrap();
    let blk = ga.decode_block(&buf, bi).unwrap();
    let s1 = blk
        .streams
        .iter()
        .find(|x| x.col == 1 && x.flags == 0x8000)
        .map(|s| s.values.clone())
        .unwrap();
    let v1 = blk
        .streams
        .iter()
        .find(|x| x.col == 1 && x.flags == 0)
        .map(|s| s.values.clone())
        .unwrap();
    let c11 = blk
        .streams
        .iter()
        .find(|x| x.col == 0xc11 && x.flags == 0)
        .map(|s| s.values.clone())
        .unwrap_or_default();
    println!("starts {} vals {} c11 {}", s1.len(), v1.len(), c11.len());
    for n in &needles {
        for oi in 0..s1.len().saturating_sub(1) {
            let (st, en) = (s1[oi] as usize, s1[oi + 1] as usize);
            if st < en && en <= v1.len() && v1[st..en].contains(n) {
                println!(
                    "needle {n}: owner ord {oi} elem {} window {:?} c11@ord {} {:?}",
                    blk.elem_start + oi as u32,
                    &v1[st..en],
                    oi,
                    c11.get(oi)
                );
            }
        }
    }
}
