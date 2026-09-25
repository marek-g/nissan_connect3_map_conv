//! nl_sim: device city-screen diagnostic. Calibrates on the stock card files, then
//! runs the freshly generated set (and mixed combos) through the identical gates.
//! Usage: cargo run --release --manifest-path src/lid_format/Cargo.toml --example sim_city_screen

use lid_format::device_sim::{Candidate, CityScreen};

const POL: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";
const TMP: &str = "/tmp/opencode/";

fn rd(p: &str) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("read {p}: {e}"))
}

fn run(label: &str, city: &[u8], rel3: &[u8], districts: &[u32]) -> Vec<(u32, Vec<Candidate>)> {
    println!("== {label}");
    match CityScreen::new(city, rel3, 0) {
        Err(lid_format::device_sim::Gate(g)) => {
            println!("   FILE LOAD GATE FAIL: {g}");
            Vec::new()
        }
        Ok(sim) => {
            let (res, log) = sim.populate(districts);
            for msg in log {
                println!("   log: {msg}");
            }
            let mut total = 0usize;
            for (d, cands) in &res {
                total += cands.len();
                let names: Vec<&str> = cands.iter().map(|c| c.name.as_str()).collect();
                println!(
                    "   district {d:>4}: {} cand [{}] {}",
                    cands.len(),
                    CityScreen::letters(&cands.iter().collect::<Vec<_>>())
                        .chars()
                        .take(12)
                        .collect::<String>(),
                    names.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
                );
            }
            println!("   TOTAL candidates: {total}");
            res
        }
    }
}

fn main() {
    dbg_pairs();
    // ground-truth districts = the source ids our generated REL00003 actually pairs
    let our_rel3_raw = rd(&format!("{TMP}REL00003.DAT"));
    let ridx = lid_format::rel::RelIndex::parse(&our_rel3_raw).expect("parse our rel3");
    let mut districts: Vec<u32> = ridx
        .d
        .iter()
        .next()
        .map(|_| {
            (0..ridx.d[2])
                .map(|s| (s, s + 1))
                .flat_map(|(lo, hi)| {
                    lid_format::rel::get_relations(&our_rel3_raw, &ridx, true, lo, hi)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|(s, _)| s)
                        .collect::<Vec<_>>()
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        })
        .unwrap();
    districts.extend([31u32, 32]); // regions seen as sources in some regions' lists
    println!("districts under test: {districts:?}");

    let stock_city = rd(&format!("{POL}LID20001.DAT"));
    let stock_rel3 = rd(&format!("{POL}REL00003.DAT"));
    let our_city = rd(&format!("{TMP}zzcity10.DAT"));

    let cal = run(
        "CALIBRATION: stock LID20001 + stock REL00003",
        &stock_city,
        &stock_rel3,
        &districts,
    );
    assert!(
        cal.iter().map(|(_, c)| c.len()).sum::<usize>() > 100,
        "calibration must reproduce a populated stock screen"
    );

    let mix1 = run(
        "MIX A: our LID20001 + stock REL00003 (old card build)",
        &our_city,
        &stock_rel3,
        &districts,
    );
    println!(
        "   old-build candidates: {} (card showed: names on type-ahead)",
        mix1.iter().map(|(_, c)| c.len()).sum::<usize>()
    );

    let our = run(
        "OURS: our LID20001 + our REL00003",
        &our_city,
        &our_rel3_raw,
        &districts,
    );
    let n_our: usize = our.iter().map(|(_, c)| c.len()).sum();
    println!("   new-build candidates: {n_our}");

    let names: Vec<&str> = our
        .iter()
        .flat_map(|(_, c)| c.iter())
        .map(|c| c.name.as_str())
        .collect();
    for want in ["WARSZAWA", "GDAŃSK", "SZCZECIN"] {
        assert!(
            names.contains(&want),
            "expected {want} in sim candidates, got {names:?}"
        );
    }
    println!("SIM GREEN: our set populates the city screen in the device model");
}

#[allow(dead_code)]
fn dbg_pairs() {
    let stock_rel3 = rd(&format!("{POL}REL00003.DAT"));
    let s = lid_format::device_sim::RelSim::open("REL00003", &stock_rel3).expect("open");
    println!("d={:?} geo={:?}", s.d, s.geo);
    for id in [457u32, 459, 111] {
        match s.per_index_by_source(&[id]) {
            Ok(p) => println!("relsim id {id}: {} pairs {p:?}", p.len()),
            Err(g) => println!("relsim id {id}: GATE {}", g.0),
        }
        let r = lid_format::rel::RelIndex::parse(&stock_rel3).unwrap();
        let v = lid_format::rel::get_relations(&stock_rel3, &r, true, id, id + 1).unwrap();
        println!("rel.rs  id {id}: {} pairs {v:?}", v.len());
    }
}
