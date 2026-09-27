//! Device-sim regression for the UI-walk replica (`walk_ui`, verified 2026-09-26 against
//! `bAddNewInputStep` 00cf7130 / `enGetEdgeNoOfElements` 00cf6240 / `enGetEdgeTargetNode`
//! 00cdbd8c): the generated city/street files must produce exactly the element domain, TEI
//! count and keyboard letters the address-search UI needs (10 cities / 9 letters,
//! 6 streets / 6 letters), with zero interval contradictions.

use lid_format::device_sim::{check_block_load, check_name_list_header, walk_ui};

fn walk_file(nm: &str, path: &str) -> (usize, usize, usize, usize, String, usize) {
    let b = std::fs::read(path).expect("read file");
    let h = check_name_list_header(nm, &b).expect("LoadHeader");
    let (mut dom, mut el, mut teie, mut over, mut valid) = (0, 0, 0, 0, 0);
    let mut letters = String::new();
    for bi in 0..h.nb as usize {
        let bl = check_block_load(nm, &b, &h, bi).expect("SetDataBlock");
        let w = walk_ui(&bl);
        dom += w.domain;
        el += w.elements;
        teie += w.tei_entries;
        over += w.over;
        valid += bl.valid_dest_popcount;
        for c in w.letters.chars() {
            if !letters.contains(c) {
                letters.push(c);
            }
        }
    }
    (dom, el, teie, over, letters, valid)
}

#[test]
fn city_file_walks_to_ten_cities() {
    let (dom, el, teie, over, letters, valid) =
        walk_file("v1", "../../trials/28 - LID/raw/LID20001.DAT");
    assert_eq!((dom, el, teie, over, valid), (10, 10, 10, 0, 10));
    assert_eq!(letters.chars().count(), 9);
    for c in "BGKLPRSWŁ".chars() {
        assert!(letters.contains(c), "letter {c} missing from speller");
    }
}

#[test]
fn street_file_walks_to_six_streets() {
    let (dom, el, teie, over, letters, valid) =
        walk_file("v6", "../../trials/28 - LID/raw/LID20006.DAT");
    assert_eq!((dom, el, teie, over, valid), (6, 6, 6, 0, 6));
    assert_eq!(letters.chars().count(), 6);
}

const POL: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                   CRYPTNAV/DATA/DATA/LID/CCP/POL/";

#[test]
#[ignore]
fn stock_city_walk_matches_realmachine_numbers() {
    let path = "/tmp/opencode/stock1r.DAT";
    if std::fs::metadata(path).is_err() {
        eprintln!("skip: copy stock LID20001.DAT to /tmp/opencode/stock1r.DAT first");
        return;
    }
    let (dom, _el, _teie, _over, letters, valid) = walk_file("stock", path);
    assert_eq!((dom, valid), (242265, 241269));
    assert!(letters.chars().count() >= 55);
    let (dom6, _, _, _, _, valid6) = walk_file("stock6", &format!("{POL}LID20006.DAT"));
    assert_eq!((dom6, valid6), (995093, 974658));
}
