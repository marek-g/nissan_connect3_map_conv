use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let idxs: Vec<usize> = a[1..].iter().map(|x| x.parse().unwrap()).collect();
    let ga = read_gen_attr(&buf).unwrap();
    // Build global 0xc13 table: values in block order, ordinal = descr index.
    let mut table: Vec<u32> = Vec::new();
    for bi in 0..ga.blocks.len() {
        let blk = match ga.decode_block(&buf, bi) {
            Ok(b) => b,
            Err(_) => continue,
        };
        for s in &blk.streams {
            if s.col == 0x0c11 && s.flags == 0 {
                table.extend(s.values.iter().cloned());
            }
        }
    }
    println!("0xc13 global table len {}", table.len());
    for i in &idxs {
        if *i < table.len() {
            println!("descr {i} -> owner {}", table[*i]);
        }
    }
}
