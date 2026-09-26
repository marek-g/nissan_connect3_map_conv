// Validation: replay the device HNR read path over the stock POL LID40006 (27 MB, 134 blocks).
use lid_format::device_sim::check_hnr;

const POL: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                   CRYPTNAV/DATA/DATA/LID/CCP/POL";

fn dump_cols() {
    let b = std::fs::read(format!("{POL}/LID40006.DAT")).unwrap();
    let gi = lid_format::read_gen_attr(&b).unwrap();
    let blk = gi.decode_block(&b, 0).unwrap();
    for s in &blk.streams {
        println!(
            "col {:#06x} flags {:#06x} code {:#04x} param {} bits {} vals {}",
            s.col,
            s.flags,
            s.code,
            s.param,
            s.bits.len(),
            s.values.len()
        );
    }
    for (c, f) in [(0xc01u32, 0u32), (0xc02, 0), (0xc11, 0)] {
        let v = &blk
            .streams
            .iter()
            .find(|s| s.col == c && s.flags == f)
            .unwrap()
            .values;
        println!(
            "vals {c:#06x}[0..16] = {:?}",
            v.iter().take(16).copied().collect::<Vec<u32>>()
        );
    }
    let _unused: Vec<u32> = blk
        .streams
        .iter()
        .take(6)
        .map(|s| s.values.iter().take(3).copied().collect::<Vec<u32>>()[0])
        .collect();
    let _ = _unused;
}

fn main() {
    if std::env::args().any(|a| a == "--dump") {
        dump_cols();
        return;
    }
    let t = std::time::Instant::now();
    let b = std::fs::read(format!("{POL}/LID40006.DAT")).unwrap();
    let recs = check_hnr(&b).expect("stock HNR replay must pass the device model");
    let streets: std::collections::BTreeSet<u32> = recs.iter().map(|r| r.street).collect();
    let even_cnt = recs.iter().filter(|r| r.even).count();
    let parity_fit = recs
        .iter()
        .filter(|r| (r.number % 2 == 0) == r.even)
        .count();
    println!(
        "stock LID40006: {} records over {} streets; even-bit set on {}; even-bit matches number-parity on {} ({:.2}%); {:?}",
        recs.len(),
        streets.len(),
        even_cnt,
        parity_fit,
        100.0 * parity_fit as f64 / recs.len() as f64,
        t.elapsed()
    );
    for r in recs.iter().take(8) {
        println!("  street {} number {} even={}", r.street, r.number, r.even);
    }
}
