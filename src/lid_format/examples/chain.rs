use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let elem: u32 = a[1].parse().unwrap();
    let ga = read_gen_attr(&buf).unwrap();
    for bi in 0..ga.blocks.len() {
        let blk = match ga.decode_block(&buf, bi) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if !(blk.elem_start <= elem && elem <= blk.elem_end) {
            continue;
        }
        let ord = (elem - blk.elem_start) as usize;
        println!(
            "block {bi} range {}..{} ord {ord}",
            blk.elem_start, blk.elem_end
        );
        let bit = |col: u32, fl: u32| {
            blk.streams
                .iter()
                .find(|x| x.col == col && x.flags == fl)
                .map(|s| s.bits.clone())
                .unwrap_or_default()
        };
        let val = |col: u32, fl: u32| {
            blk.streams
                .iter()
                .find(|x| x.col == col && x.flags == fl)
                .map(|s| s.values.clone())
                .unwrap_or_default()
        };
        let b12 = bit(0x0c12, 0x4000);
        let pord = b12[..ord + 1].iter().filter(|&&x| x).count() - 1;
        println!(
            "0xc12 present-owner ord {pord} of {}",
            b12.iter().filter(|&&x| x).count()
        );
        let s12 = val(0x0c12, 0x8000);
        let v12 = val(0x0c12, 0);
        let (st, en) = (
            s12[pord] as usize,
            s12.get(pord + 1).map(|&x| x as usize).unwrap_or(v12.len()),
        );
        let descrs = &v12[st..en];
        println!("0xc12 descr idxs for this owner: {descrs:?}");
        // ranges: 0xc14 exists-bit owners, 0xc14 starts, 0xc14 vals: try to map descr -> range
        let b14 = bit(0x0c14, 0x4000);
        let s14 = val(0x0c14, 0x8000);
        let v14 = val(0x0c14, 0);
        // assume 0xc14 keyed by same present-owner order? print slice window covering descr values
        let cnt14 = b14.iter().filter(|&&x| x).count();
        println!(
            "0xc14 owners={} vals={} starts={}",
            cnt14,
            v14.len(),
            s14.len()
        );
        // find where descr value appears as index into a starts-list:
        for d in descrs {
            let d = *d as usize;
            if d < v14.len() / 2 {
                println!(
                    "  descr {d} -> v14 pair ({},{})",
                    v14[2 * d],
                    v14[2 * d + 1]
                );
            }
        }
        // also 0xc11 record descrs for this owner:
        let s11 = val(0x0c11, 0x8000);
        let b11 = bit(0x0c11, 0x4000);
        let p11 = b11[..ord + 1].iter().filter(|&&x| x).count() - 1;
        if !s11.is_empty() {
            let (a1, b1) = (
                s11[p11] as usize,
                s11.get(p11 + 1)
                    .map(|&x| x as usize)
                    .unwrap_or(val(0x0c11, 0).len()),
            );
            let r11 = &val(0x0c11, 0)[a1..b1];
            println!("0xc11 record descrs: {r11:?}");
            for d in r11 {
                let d = *d as usize;
                if d / 2 < v14.len() / 2 && d % 2 == 0 {
                    println!("  r11 {d} -> pair ({},{})", v14[d], v14[d + 1]);
                }
            }
        }
    }
}
