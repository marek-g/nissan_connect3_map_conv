use lid_format::rel::{get_relations, RelIndex};
fn main() {
    for path in std::env::args().skip(1) {
        let b = std::fs::read(&path).unwrap();
        let idx = RelIndex::parse(&b).unwrap();
        println!("=== {path} : listA={} listB={} src={} tgt={}", idx.d[0], idx.d[1], idx.d[2], idx.d[3]);
        let mut by_key: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        for k in 0..idx.d[2] as u32 {
            let r = get_relations(&b, &idx, true, k, k + 1).unwrap();
            if !r.is_empty() {
                by_key.insert(k, r.iter().map(|&(_, t)| t).collect());
            }
        }
        for (k, tgts) in &by_key {
            println!("  key {k}: {tgts:?}");
        }
        println!("  total keys with rows: {}", by_key.len());
    }
}
