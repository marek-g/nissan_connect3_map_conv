use lid_format::device_sim::{check_block_load, check_name_list_header};

fn main() {
    for path in std::env::args().skip(1) {
        let b = std::fs::read(&path).unwrap();
        let short = path.rsplit('/').next().unwrap_or(&path).to_string();
        let h = match check_name_list_header(&short, &b) {
            Ok(h) => h,
            Err(g) => {
                println!("{short}: LoadHeader FAIL: {}", g.0);
                continue;
            }
        };
        let nb = h.nb as usize;
        let mut fails = Vec::new();
        for bi in 0..nb.min(if short.contains("LID20001") || short.contains("STOCK") {
            3
        } else {
            nb
        }) {
            match check_block_load(&short, &b, &h, bi) {
                Ok(bl) => {
                    println!(
                        "{short} blk{bi}: nE={} root-edges={} letters=\"{}\" validDest={}/{} danger={}",
                        bl.ne,
                        bl.root_edges.len(),
                        bl.letters,
                        bl.valid_dest_popcount,
                        bl.ne,
                        bl.danger.len()
                    );
                    for d in &bl.danger {
                        println!("   DANGER: {d}");
                    }
                    if bi == 0 {
                        let e: Vec<String> = bl
                            .root_edges
                            .iter()
                            .take(12)
                            .map(|(l, e)| format!("{l:?}->{e}"))
                            .collect();
                        println!("   edges[0..12]: {}", e.join(" "));
                    }
                }
                Err(g) => fails.push(format!("blk{bi}: {}", g.0)),
            }
        }
        for fl in fails {
            println!("{short}: SetDataBlock FAIL {fl}");
        }
    }
}
