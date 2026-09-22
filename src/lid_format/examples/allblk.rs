use lid_format::read_gen_attr;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = std::fs::read(&a[0]).unwrap();
    let ga = read_gen_attr(&buf).unwrap();
    println!("blocks {}", ga.blocks.len());
    let mut err = 0; let mut last = 0;
    for bi in 0..ga.blocks.len() {
        match ga.decode_block(&buf, bi) {
            Ok(b) => { last = bi; if b.elem_end >= 974871 { println!("tail block {bi} {}..{} streams {}", b.elem_start, b.elem_end, b.streams.len()); } }
            Err(e) => { err += 1; if bi >= 130 { println!("block {bi} ERR {e}"); } }
        }
    }
    println!("errors {err} last-ok {last}");
}
