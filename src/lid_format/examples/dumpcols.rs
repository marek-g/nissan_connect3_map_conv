// Debug: dump decoded GenAttr block streams for the block covering a given element.
// Usage: dumpcols FILE ELEM [NEEDLE ...]
use lid_format::read_gen_attr;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let elem: u32 = a[1].parse().unwrap();
    let needles: Vec<u32> = a[2..].iter().map(|x| x.parse().unwrap()).collect();
    let ga = read_gen_attr(&buf).unwrap();
    for bi in 0..ga.blocks.len() {
        let blk = match ga.decode_block(&buf, bi) {
            Ok(b) => b,
            Err(_) => continue,
        };
        if !(blk.elem_start <= elem && elem <= blk.elem_end) {
            continue;
        }
        println!("block {bi} range {}..{}", blk.elem_start, blk.elem_end);
        for s in &blk.streams {
            print!(
                "col {:#06x} flags {:#06x} vals={} bits={}",
                s.col,
                s.flags,
                s.values.len(),
                s.bits.len()
            );
            if !s.values.is_empty() {
                print!(" head {:?}", &s.values[..s.values.len().min(8)]);
            }
            for n in &needles {
                if let Some(p) = s.values.iter().position(|&v| v == *n) {
                    print!("  NEEDLE {n:#x} at idx {p}");
                }
                if let Some(p) = s.bits.iter().position(|&x| (x as u32) == *n) {
                    print!("  BITNEEDLE {n} at {p}");
                }
            }
            println!();
        }
    }
}
