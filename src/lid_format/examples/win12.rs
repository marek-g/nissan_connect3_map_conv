use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let elem: u32 = a[1].parse().unwrap();
    let needles: Vec<u32> = a[2..].iter().map(|x| x.parse().unwrap()).collect();
    let ga = read_gen_attr(&buf).unwrap();
    // global record->owner table, sparse-expanded (values packed by existence bits)
    let mut owner: Vec<u32> = Vec::new();
    for bi in 0..ga.blocks.len() {
        let blk = match ga.decode_block(&buf,bi) { Ok(b)=>b, Err(_)=>continue };
        let n = (blk.elem_end - blk.elem_start + 1) as usize; // placeholder, fixed below
        let _ = n;
        let bits = blk.streams.iter().find(|x| x.col==0x0c11 && x.flags==0x4000).map(|s|s.bits.clone()).unwrap_or_default();
        let vals = blk.streams.iter().find(|x| x.col==0x0c11 && x.flags==0).map(|s|s.values.clone()).unwrap_or_default();
        // owner-index col is keyed per hnr RECORD, not per element; records count = bits len
        let mut t: Vec<u32> = vec![0xffffffff; bits.len()];
        if bits.iter().any(|&x| x) { let mut vi=0; for (i,&b) in bits.iter().enumerate() { if b && vi<vals.len() { t[i]=vals[vi]; vi+=1; } } }
        owner.extend(t);
    }
    println!("owner table len {}", owner.len());
    for bi in 0..ga.blocks.len() {
        let blk = match ga.decode_block(&buf, bi) { Ok(b)=>b, Err(_)=>continue };
        if !(blk.elem_start<=elem && elem<=blk.elem_end) { continue }
        let ord=(elem-blk.elem_start) as usize;
        let bit=|c:u32,f:u32| blk.streams.iter().find(|x| x.col==c&&x.flags==f).map(|s|s.bits.clone()).unwrap_or_default();
        let val=|c:u32,f:u32| blk.streams.iter().find(|x| x.col==c&&x.flags==f).map(|s|s.values.clone()).unwrap_or_default();
        let b12=bit(0x0c12,0x4000);
        if b12.is_empty() || !b12[ord] { println!("elem {elem}: no 0xc12 owner"); continue }
        let pord=b12[..ord+1].iter().filter(|&&x|x).count()-1;
        let s12=val(0x0c12,0x8000); let v12=val(0x0c12,0);
        let (st,en)=(s12[pord] as usize, s12.get(pord+1).map(|&x|x as usize).unwrap_or(v12.len()));
        let d=&v12[st..en];
        let ow:Vec<u32>=d.iter().map(|&x| *owner.get(x as usize).unwrap_or(&0xdeadbeef)).collect();
        let hit:Vec<u32>=ow.iter().cloned().filter(|v| needles.contains(v)).collect();
        println!("pord {pord} descrs {:?} owners {:?} {}", d, ow, if hit.is_empty() {String::new()} else {format!("HIT {hit:?}")});
    }
}
