// Scan all GenAttr blocks: report (block,col,flags,idx) where any needle value occurs.
use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let needles: Vec<u32> = a[1..].iter().map(|x| x.parse().unwrap()).collect();
    let ga = read_gen_attr(&buf).unwrap();
    for bi in 0..ga.blocks.len() {
        let blk = match ga.decode_block(&buf, bi) {
            Ok(b) => b,
            Err(_) => continue,
        };
        for s in &blk.streams {
            for n in &needles {
                let mut cnt = 0usize;
                let mut first = usize::MAX;
                for (i, &v) in s.values.iter().enumerate() {
                    if v == *n {
                        if first == usize::MAX {
                            first = i
                        }
                        cnt += 1;
                    }
                }
                if cnt > 0 {
                    println!(
                        "blk {bi} {}..{} col {:#06x} fl {:#06x} needle {n} x{cnt} first@{first}",
                        blk.elem_start, blk.elem_end, s.col, s.flags
                    );
                }
            }
        }
    }
}
