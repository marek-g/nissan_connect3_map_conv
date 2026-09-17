#[test]
#[ignore]
fn probe_raw_cpr_streams() {
    for path in std::env::var("PROBE_FILES").unwrap().split(':') {
        let b = std::fs::read(path).unwrap();
        for off in 0x00..0x90 {
            let tail = &b[off..];
            let v = u16::from_le_bytes(tail[..2].try_into().unwrap());
            if v != 5 { continue; }
            match cprnav::decompress(tail) {
                Ok(out) => println!("{path} @{off:#x}: Decomp OK -> {} bytes, head={:?}", out.len(), &out[..out.len().min(24)]),
                Err(e) => println!("{path} @{off:#x}: v5 head but fail: {}", &e[..e.len().min(90)]),
            }
        }
    }
}
