use lid_format::rel::{get_relations, RelIndex};
fn main() {
    let rel = std::fs::read("/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00000.DAT").unwrap();
    let idx = RelIndex::parse(&rel).unwrap();
    let r = get_relations(&rel, &idx, true, 233478, 233479).unwrap();
    let ts: Vec<u32> = r.iter().map(|&(_,t)| t).collect();
    let mut sorted = ts.clone(); sorted.sort(); sorted.dedup();
    let contiguous = sorted.iter().enumerate().all(|(i,&v)| v == i as u32);
    println!("UZARZEWO: n={} min={} max={} unique={} contiguous_from_0={}", ts.len(), ts.iter().min().unwrap(), ts.iter().max().unwrap(), sorted.len(), contiguous);
    // sample village->city pairs
    for s in [195864u32, 208822, 241261, 241239, 233746] {
        let r = get_relations(&rel, &idx, true, s, s + 1).unwrap();
        println!("src {s}: {} targets: {:?}", r.len(), r.iter().map(|&(_,t)| t).collect::<Vec<_>>());
    }
}
