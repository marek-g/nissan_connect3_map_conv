use lid_format::device_sim::check_name_list_header;

fn main() {
    for path in std::env::args().skip(1) {
        let b = std::fs::read(&path).unwrap();
        match check_name_list_header(&path, &b) {
            Ok(h) => {
                println!("== {path}");
                println!(
                    "elem={} nb={} nrec={} nrel={} shift={} origin={:?} blob_end={:#x}",
                    h.elem, h.nb, h.nrec, h.nrel, h.shift, h.origin, h.blob_end
                );
                for (i, s) in h.streams.iter().enumerate() {
                    let head: Vec<String> = s.iter().take(24).map(|v| format!("{v}")).collect();
                    println!(
                        "  s{i} len={} : {}{}",
                        s.len(),
                        head.join(","),
                        if s.len() > 24 { ",..." } else { "" }
                    );
                }
            }
            Err(g) => println!("== {path}: GATE FAIL: {}", g.0),
        }
    }
}
