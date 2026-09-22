use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let needles: Vec<u32> = a[1..].iter().map(|x| x.parse().unwrap()).collect();
    let ga = read_gen_attr(&buf).unwrap();
    let blk = ga.decode_block(&buf, 8).unwrap();
    let bits = blk.streams.iter().find(|x| x.col==0x0c01&&x.flags==0x4000).map(|s|s.bits.clone()).unwrap();
    let s01 = blk.streams.iter().find(|x| x.col==0x0c01&&x.flags==0x8000).map(|s|s.values.clone()).unwrap_or_default();
    let v01 = blk.streams.iter().find(|x| x.col==0x0c01&&x.flags==0).map(|s|s.values.clone()).unwrap_or_default();
    println!("0xc01: bits {} starts {} vals {}", bits.len(), s01.len(), v01.len());
    // owner ordinals (present bits) - for CZERNA pord 238 show its start index
    let ord_elems: Vec<u32> = (blk.elem_start..=blk.elem_end).collect();
    for ord in [1346usize/*67687*/, 1337/*67678*/] {
        let p = bits[..=ord].iter().filter(|&&x|x).count()-1;
        let (st,en) = (s01[p] as usize, s01[p+1] as usize);
        println!("elem {} pord {p} c01 vals {:?} needles {:?}", blk.elem_start+ord as u32, &v01[st..en], {let h:Vec<u32>=v01[st..en].iter().cloned().filter(|v|needles.contains(v)).collect(); h});
    }
}
