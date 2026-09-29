// Probe: for a REL file, report per source key the target count + whether it contains given probe ids.
// Usage: tmp_rel6_probe <rel.DAT> [probe_id ...]
use lid_format::rel::{get_relations, RelIndex};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = &args[0];
    let probes: Vec<u32> = args.iter().skip(1).filter_map(|s| s.parse().ok()).collect();
    let b = std::fs::read(path).unwrap();
    let idx = RelIndex::parse(&b).unwrap();
    println!("=== {path} : listA={} listB={} src={} tgt={}", idx.d[0], idx.d[1], idx.d[2], idx.d[3]);
    let mut any_key_with_probe = 0;
    for k in 0..idx.d[2] as u32 {
        let r = get_relations(&b, &idx, true, k, k + 1).unwrap();
        let tgts: Vec<u32> = r.iter().map(|&(_, t)| t).collect();
        let hits: Vec<u32> = probes.iter().copied().filter(|p| tgts.contains(p)).collect();
        if !hits.is_empty() {
            any_key_with_probe += 1;
            println!("  key {k:>2}: {} cities, PROBE hits = {hits:?}", tgts.len());
        } else {
            println!("  key {k:>2}: {} cities", tgts.len());
        }
    }
    println!("keys containing any probe id: {any_key_with_probe}/{}", idx.d[2]);
}
