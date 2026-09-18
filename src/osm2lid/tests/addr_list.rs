// osm2lid LID20000 (listID-129 address gazetteer) integration: run the bin on the addrs fixture,
// then validate the emitted file against the stock row grammar (LID_format §11.2, stock POL oracle):
// plain `CITY` row per city, one `CITY, STREET` row per (city, street) pair, one
// `CITY, STREET NUMBER` row per house number, unique names, raw number text preserved (`11A`).

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

#[test]
fn addr_list_grammar_and_join() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let out = Path::new(manifest).join("tests/out_addr_list");
    let _ = std::fs::remove_dir_all(&out);
    let inp = format!("{}/tests/data/addrs.osm", manifest);
    let outdir = out.to_str().unwrap().to_string();
    let status = Command::new(env!("CARGO_BIN_EXE_osm2lid"))
        .args([&inp, "-o", &outdir])
        .status()
        .expect("run osm2lid");
    assert!(status.success(), "osm2lid failed");

    let bytes = std::fs::read(out.join("LID20000.DAT")).expect("LID20000");
    // outer header: rIdxListID { u16 region; u16 listID; ... } — the device binds the file by these.
    assert_eq!(&bytes[0x00..0x02], &0x0402u16.to_le_bytes(), "regionIdent");
    assert_eq!(&bytes[0x02..0x04], &129u16.to_le_bytes(), "listID 129");
    assert_eq!(&bytes[0x0c..0x10], &1u32.to_le_bytes(), "kind = raw name list");

    let nl = lid_format::read(&bytes).expect("decode LID20000");
    let rows: BTreeSet<&str> = nl.elements.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(rows.len(), nl.elements.len(), "stock: every 129 name is unique");

    let city_nl = lid_format::read(&std::fs::read(out.join("LID20001.DAT")).unwrap()).unwrap();
    let cities: BTreeSet<&str> = city_nl.elements.iter().map(|e| e.name.as_str()).collect();
    let street_nl = lid_format::read(&std::fs::read(out.join("LID20006.DAT")).unwrap()).unwrap();
    let streets: BTreeSet<&str> = street_nl.elements.iter().map(|e| e.name.as_str()).collect();

    // Every row must join back: city part == a LID20001 element name; street part == a LID20006
    // element name (byte-identical, the device re-derives street indices by string equality).
    for name in &rows {
        let (city, rest) = match name.split_once(", ") {
            Some((c, r)) => (c, Some(r)),
            None => (*name, None),
        };
        assert!(cities.contains(city), "city element missing for row {name:?}");
        if let Some(rest) = rest {
            let street = streets
                .iter()
                .filter(|&&s| s == rest || rest.starts_with(&format!("{s} ")))
                .max_by_key(|s| s.len());
            assert!(street.is_some(), "street element missing for row {name:?}");
        }
    }

    // Exact fixture expectation (addrs.osm): Ceszin/Marszalkowska 10+11+11A, Bemow/Nowogrodzka 5.
    for want in [
        "Ceszin",
        "Ceszin, Marszalkowska",
        "Ceszin, Marszalkowska 10",
        "Ceszin, Marszalkowska 11",
        "Ceszin, Marszalkowska 11A",
        "Bemow",
        "Bemow, Nowogrodzka",
        "Bemow, Nowogrodzka 5",
    ] {
        assert!(rows.contains(want), "missing row {want:?}, got {rows:?}");
    }
    assert_eq!(rows.len(), 8, "no extra rows expected on this fixture");
    // Gazetteer flavor: no coordinate stream (§12.5 / stock file-level origin -1/-1).
    assert!(
        nl.elements.iter().all(|e| !e.has_pos),
        "LID20000 must carry no positions"
    );
}
