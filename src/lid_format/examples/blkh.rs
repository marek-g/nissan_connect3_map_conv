use std::fs;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let buf = fs::read(&a[0]).unwrap();
    // name-list TOC: header @0? dump first 0x40 bytes and last TOC entries heuristically:
    // print le32 at 0..0x40
    for i in (0..0x40).step_by(4) {
        println!("{i:#04x}: {:08x}", u32::from_le_bytes(buf[i..i+4].try_into().unwrap()));
    }
}
