use lid_format::rel::{get_relations, RelIndex};
use std::collections::BTreeMap;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let path = &a[1];
    let bys: bool = a[2] == "src";
    let filt: Option<Vec<u32>> = a
        .get(3)
        .map(|f| f.split(',').map(|x| x.parse().unwrap()).collect());
    let b = std::fs::read(path).unwrap();
    let idx = RelIndex::parse(&b).unwrap();
    println!("d = {:?} by_source={bys}", idx.d);
    let (lo, hi) = (0, if bys { idx.d[2] } else { idx.d[3] });
    let rows = get_relations(&b, &idx, bys, lo, hi).unwrap();
    println!("rows = {}", rows.len());
    let mut by_k: BTreeMap<u32, (usize, u32, u32, Vec<u32>)> = BTreeMap::new();
    for (k, v) in &rows {
        let e = by_k.entry(*k).or_insert((0, *v, *v, Vec::new()));
        e.0 += 1;
        e.1 = e.1.min(*v);
        e.2 = e.2.max(*v);
        if e.3.len() < 12 {
            e.3.push(*v);
        }
    }
    println!("distinct filtered keys = {}", by_k.len());
    for (k, (c, mn, mx, s)) in by_k.iter() {
        match &filt {
            Some(f) if f.contains(k) => {
                println!("KEY {k} -> {c} rows range [{mn}..{mx}]");
                for (kk, vv) in &rows {
                    if kk == k {
                        print!("{vv} ");
                    }
                }
                println!();
            }
            Some(_) => {}
            None => println!("key {k} -> {c} rows range [{mn}..{mx}] head {s:?}"),
        }
    }
}
