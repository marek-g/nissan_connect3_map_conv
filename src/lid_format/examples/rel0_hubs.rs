use lid_format::rel::{get_relations, RelIndex};
use std::collections::BTreeMap;

fn main() {
    let fw = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";
    let b = std::fs::read(format!("{fw}REL00000.DAT")).unwrap();
    let idx = RelIndex::parse(&b).unwrap();
    println!("d = {:?}", idx.d);
    let rows = get_relations(&b, &idx, true, 0, idx.d[2]).unwrap();
    println!("rows = {}", rows.len());
    let mut by_src: BTreeMap<u32, usize> = BTreeMap::new();
    for (s, _) in &rows {
        *by_src.entry(*s).or_default() += 1;
    }
    let mut top: Vec<(u32, usize)> = by_src.iter().map(|(k, v)| (*k, *v)).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    println!("distinct sources with rows = {}", by_src.len());
    for (s, c) in top.iter().take(25) {
        println!("src {s:#06x} = {s} -> {c} targets");
    }
}
