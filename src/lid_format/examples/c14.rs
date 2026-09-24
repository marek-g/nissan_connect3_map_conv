use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let elem: u32 = a[1].parse().unwrap();
    let needles: Vec<u32> = a[2..].iter().map(|x| x.parse().unwrap()).collect();
    let ga = read_gen_attr(&buf).unwrap();
    // global record space = cumulative block widths? records = rows of 0xc01 VL per block
    let mut rec_base = 0usize;
    for bi in 0..ga.blocks.len() {
        let blk = match ga.decode_block(&buf, bi) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let bits = blk
            .streams
            .iter()
            .find(|x| x.col == 0x0c01 && x.flags == 0x4000)
            .map(|s| s.bits.clone())
            .unwrap_or_default();
        let nrec = bits.iter().filter(|&&x| x).count(); // present owners -> their records
        if blk.elem_start <= elem && elem <= blk.elem_end {
            let b14 = blk
                .streams
                .iter()
                .find(|x| x.col == 0x0c14 && x.flags == 0x4000)
                .map(|s| s.bits.clone())
                .unwrap_or_default();
            let s14 = blk
                .streams
                .iter()
                .find(|x| x.col == 0x0c14 && x.flags == 0x8000)
                .map(|s| s.values.clone())
                .unwrap_or_default();
            let v14 = blk
                .streams
                .iter()
                .find(|x| x.col == 0x0c14 && x.flags == 0)
                .map(|s| s.values.clone())
                .unwrap_or_default();
            let pord = if !b14.is_empty() {
                let ord = (elem - blk.elem_start) as usize;
                b14[..=ord.min(b14.len() - 1)]
                    .iter()
                    .filter(|&&x| x)
                    .count()
                    .saturating_sub(1)
            } else {
                usize::MAX
            };
            println!(
                "block {bi} records={nrec} b14 len {} pord {pord}",
                b14.len()
            );
            if pord != usize::MAX && pord < s14.len() - 1 {
                let (st, en) = (s14[pord] as usize, s14[pord + 1] as usize);
                println!(
                    "c14 window: {:?}",
                    &v14[st.min(v14.len())..en.min(v14.len())]
                );
            }
            // dump 0xc14 head and where needles live globally
            for n in &needles {
                if let Some(p) = v14.iter().position(|&x| x == *n) {
                    println!("NEEDLE {n} at idx {p} (block c14 vals)");
                }
            }
        }
        rec_base += nrec;
        let _ = rec_base;
    }
}
