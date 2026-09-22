use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let bi: usize = a[1].parse().unwrap();
    let ords: Vec<usize> = a[2..].iter().map(|x| x.parse().unwrap()).collect();
    let ga = read_gen_attr(&buf).unwrap();
    let blk = ga.decode_block(&buf, bi).unwrap();
    let g=|c:u32,f:u32| blk.streams.iter().find(|x| x.col==c&&x.flags==f).map(|s|s.values.clone()).unwrap_or_default();
    for oi in ords {
        let win=|c:u32| { let (s,v)=(g(c,0x8000),g(c,0)); if oi+1<s.len() && (s[oi+1] as usize)<=v.len() { v[s[oi] as usize..s[oi+1] as usize].to_vec() } else { vec![] } };
        println!("ord {oi} elem {}: col1 {:?} c01 {:?} c12 {:?}", blk.elem_start+oi as u32, win(1), win(0xc01), win(0xc12));
    }
    let c11 = g(0xc11,0); println!("c11[683..695]: {:?}", &c11[683..695.min(c11.len())]);
}
