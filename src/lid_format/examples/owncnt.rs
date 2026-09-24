use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let ga = read_gen_attr(&buf).unwrap();
    let (mut with, mut without) = (0, 0);
    let mut missing = vec![];
    for bi in 134..ga.blocks.len() {
        let blk = ga.decode_block(&buf, bi).unwrap();
        let cb1 = blk
            .streams
            .iter()
            .find(|x| x.col == 0xc01 && x.flags == 0x4000)
            .map(|s| s.bits.clone())
            .unwrap();
        let ob = blk
            .streams
            .iter()
            .find(|x| x.col == 1 && x.flags == 0x4000)
            .map(|s| s.bits.clone())
            .unwrap_or_default();
        for ord in 0..cb1.len() {
            if cb1[ord] {
                let has = !ob.is_empty() && ob[ord];
                if has {
                    with += 1
                } else {
                    without += 1;
                    missing.push(blk.elem_start + ord as u32);
                }
            }
        }
    }
    println!("with {with} without {without}: {missing:?}");
}
