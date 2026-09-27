use lid_format::device_sim::*;
fn main() {
    for (nm, path) in [
        ("STOCK", "/tmp/opencode/stock1r.DAT"),
        ("OURS", "trials/28 - LID/raw/LID20001.DAT"),
    ] {
        let b = std::fs::read(path).unwrap();
        let h = check_name_list_header(nm, &b).unwrap();
        println!("{nm}: nrec={} blob_end={}", h.nrec, h.blob_end);
        let s2 = &h.streams[2];
        let s3 = &h.streams[3];
        for i in 0..s2.len().min(6) {
            println!(
                "  rec{i}: start={} end={} (len {})",
                s2[i],
                s3[i],
                s3[i].wrapping_sub(s2[i])
            );
        }
        let n = s2.len();
        if n > 3 {
            println!("  rec{}: start={} end={}", n - 1, s2[n - 1], s3[n - 1]);
        }
    }
}
