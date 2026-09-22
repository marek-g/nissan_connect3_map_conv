use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let bi: usize = a[1].parse().unwrap();
    let col: u32 = u32::from_str_radix(&a[2],16).unwrap();
    let fl: u32 = u32::from_str_radix(&a[3],16).unwrap();
    let ga = read_gen_attr(&buf).unwrap();
    let blk = ga.decode_block(&buf, bi).unwrap();
    if let Some(s)=blk.streams.iter().find(|x| x.col==col&&x.flags==fl) {
        println!("len {}", s.values.len());
        println!("{:?}", &s.values[..s.values.len().min(60)]);
    } else { println!("none"); }
}
