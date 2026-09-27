use lid_format::device_sim::*;
use std::collections::BTreeSet;

fn run(nm: &str, path: &str) {
    let b = std::fs::read(path).unwrap();
    let h = check_name_list_header(nm, &b).unwrap();
    let (mut dom, mut el, mut teie, mut over, mut valid) = (0, 0, 0, 0, 0);
    let mut chars = BTreeSet::new();
    let nb = h.nb;
    for bi in 0..h.nb as usize {
        let bl = check_block_load(nm, &b, &h, bi).unwrap();
        let w = walk_ui(&bl);
        dom += w.domain;
        el += w.elements;
        teie += w.tei_entries;
        over += w.over;
        valid += bl.valid_dest_popcount;
        for c in w.letters.chars() {
            chars.insert(c);
        }
    }
    let cs: String = chars.iter().take(14).collect();
    println!("{nm}: nb={nb} domain={dom} TEI={el} teiEntries={teie} over={over} valid040d={valid} letters='{cs}...'");
}

fn main() {
    let fw = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";
    run("STOCK-1", "/tmp/opencode/stock1r.DAT");
    run("STOCK-6", &format!("{fw}LID20006.DAT"));
    run("V1    1", "trials/28 - LID/raw/LID20001.DAT");
    run("V1    6", "trials/28 - LID/raw/LID20006.DAT");
}
