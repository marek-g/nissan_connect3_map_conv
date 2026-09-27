use lid_format::device_sim::*;

fn main() {
    let fw = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";
    let ours = std::fs::read("trials/28 - LID/raw/LID20001.DAT").unwrap();
    let streets = std::fs::read(format!("{fw}LID20006.DAT")).unwrap();
    let s0 = std::fs::read(format!("{fw}REL00000.DAT")).unwrap();
    let s1 = std::fs::read(format!("{fw}REL00001.DAT")).unwrap();
    let o0 = std::fs::read("trials/28 - LID/raw/REL00000.DAT").unwrap();
    let o1 = std::fs::read("trials/28 - LID/raw/REL00001.DAT").unwrap();
    // Name-keyed street seeding from LID20000 (listID 129): rows "CITY, STREET NUMBER".
    // Same for V1 and 2b (mechanism ignores element ids) - identical predictions by design.
    let hnr = std::fs::read_to_string("/tmp/opencode/hnr_all.json")
        .map(|s| parse_names(&s))
        .unwrap_or_default();
    println!("hnr address rows loaded: {}", hnr.len());
    for (i, nm) in [
        "BYDGOSZCZ",
        "GDAŃSK",
        "KRAKÓW",
        "LUBLIN",
        "POZNAŃ",
        "RADOM",
        "SZCZECIN",
        "WARSZAWA",
        "WROCŁAW",
        "ŁÓDŹ",
    ]
    .into_iter()
    .enumerate()
    {
        let (r, s) = city_street_rows(&hnr, nm);
        println!("hnr-streets {i} {nm}: rows={r} streets={s} (V1==2b: name-keyed)");
    }
    println!("city streets(V1=stockREL1 2b=ourREL1) districts(V1=stockREL0 2b=ourREL0)");
    let stok = fmt(check_city_streets(&streets, &s1, 208837).map(|r| r.list.len()));
    for c in 0..10u32 {
        let v1s = check_city_streets(&streets, &s1, c).map(|r| r.list.len());
        let b2s = check_city_streets(&streets, &o1, c).map(|r| r.list.len());
        let v1d = check_city_districts(&ours, &s0, c).map(|r| r.list.len());
        let b2d = check_city_districts(&ours, &o0, c).map(|r| r.list.len());
        println!(
            "city {c}: streets V1={} 2b={} (stockKRAKOW-ref={}) | districts V1={} 2b={}",
            fmt(v1s),
            fmt(b2s),
            stok,
            fmt(v1d),
            fmt(b2d)
        );
    }
}

fn fmt(r: Result<usize, Gate>) -> String {
    match r {
        Ok(n) => n.to_string(),
        Err(e) => format!("GATE({e:?})"),
    }
}

fn parse_names(blob: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in blob.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("\"name\": \"") {
            let rest = rest.trim_end_matches(',').trim_end_matches('"');
            out.push(rest.to_string());
        }
    }
    out
}
