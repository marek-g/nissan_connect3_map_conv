use lid_format::device_sim::*;
fn show(nm: &str, path: &str, do_blocks: bool) {
    let b = std::fs::read(path).unwrap();
    let h = check_name_list_header(nm, &b).unwrap();
    println!(
        "{nm}: elem={} nb={} nrec={} nrel={} shift={:#x} rel={:?} cats={:?}",
        h.elem, h.nb, h.nrec, h.nrel, h.shift, h.rel_files, h.cats
    );
    if do_blocks {
        let mut letters = std::collections::BTreeSet::new();
        let mut edges = 0;
        let mut pop = 0;
        let mut bad = 0;
        for bi in 0..h.nb as usize {
            match check_block_load(nm, &b, &h, bi) {
                Err(e) => {
                    bad += 1;
                    if bad < 4 {
                        println!("  blk{bi}: {e:?}");
                    }
                }
                Ok(bl) => {
                    edges += bl.root_edges.len();
                    pop += bl.valid_dest_popcount;
                    for c in bl.letters.chars() {
                        letters.insert(c);
                    }
                }
            }
        }
        println!(
            "  blocks ok={} bad={} pop={} letters={}",
            h.nb as usize - bad,
            bad,
            pop,
            letters.iter().collect::<String>()
        );
    }
}
fn main() {
    let fw = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";
    show("STOCK-1", "/tmp/opencode/stock1r.DAT", true);
    show("STOCK-6", &format!("{fw}LID20006.DAT"), true);
    show("OURS-1", "trials/28 - LID/raw/LID20001.DAT", true);
    show("OURS-6", "trials/28 - LID/raw/LID20006.DAT", true);
}
