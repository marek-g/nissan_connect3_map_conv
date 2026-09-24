// Dump the extra sub-header sections of a city-list (LID20001) file with their stream codecs.
// Usage: secdump FILE
use lid_format::decode_u32_probe;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let d = std::fs::read(&a[0]).unwrap();
    let hdr = 0x77usize;
    let extra = u32::from_le_bytes(d[0x14..0x18].try_into().unwrap()) as usize;
    let na = u16::from_le_bytes(d[hdr + 4..hdr + 6].try_into().unwrap()) as usize;
    let end = hdr + extra;
    let mut bounds = Vec::new();
    for i in 0..7 {
        bounds.push(
            u32::from_le_bytes(d[hdr + 25 + 5 * i..hdr + 29 + 5 * i].try_into().unwrap()) as usize,
        );
    }
    bounds.push(end);
    for i in 0..7 {
        let code = d[hdr + 24 + 5 * i] as u32;
        let s = bounds[i];
        let e = bounds[i + 1];
        print!("sec{i} code {code:#x} len {} ", e - s);
        let nmax = (e - s) * 12;
        let (v, used) = decode_u32_probe(&d, code, s, e, nmax.min(2000));
        println!(
            "decoded {} vals, consumed {}/{} bytes",
            v.len(),
            used,
            e - s
        );
        let show = v.len().min(50);
        println!("  vals[..{show}] = {v:?}");
        if v.len() > show {
            println!("  vals[-8..] = {:?}", &v[v.len() - 8..]);
        }
    }
    println!("blocks {na}");
}
