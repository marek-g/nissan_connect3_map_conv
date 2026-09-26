// Build sample streets (LID20006 shape) + REL00001 owner matrix, verify with the device sim.
// usage: streets  -> writes /tmp/opencode/zzstreet6.DAT + REL00001_streets.DAT
use lid_format::city::CityEntry;
use lid_format::device_sim::{
    check_block_load, check_hnr, check_name_list_header, check_street_list,
};
use lid_format::street::{build_hnr_file, build_street_file, build_street_relations, HnrRange};

const POL: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                   CRYPTNAV/DATA/DATA/LID/CCP/POL";

fn main() {
    // (name, city element id in trials zzcity10 set, lon, lat)
    let specs = [
        ("EGZAMINACYJNA", 4u32, 16.9252, 52.4064), // POZNAŃ
        ("PRZYKŁADOWA", 2, 19.9450, 50.0647),      // KRAKÓW
        ("SIESTA", 7, 21.0118, 52.2298),           // WARSZAWA
        ("TESTOWA", 7, 21.0218, 52.2398),          // WARSZAWA
        ("WIDMOWA", 1, 18.6464, 54.3520),          // GDAŃSK
        ("ZABAWNA", 9, 19.4560, 51.7592),          // ŁÓDŹ
    ];
    let entries: Vec<CityEntry> = specs
        .iter()
        .map(|(n, _, lo, la)| CityEntry::from_deg(n, *lo, *la))
        .collect();
    let stock = std::fs::read(format!("{POL}/LID20006.DAT")).unwrap();
    let (data, origin) = build_street_file(&entries, &stock);
    std::fs::write("/tmp/opencode/zzstreet6.DAT", &data).unwrap();
    let rel1 = build_street_relations(
        &specs.iter().map(|&(_, c, _, _)| c).collect::<Vec<u32>>(),
        10,
    )
    .unwrap();
    std::fs::write("/tmp/opencode/REL00001_streets.DAT", &rel1).unwrap();

    // ---- device simulator pass ----
    let h = check_name_list_header("zzstreet6.DAT", &data).expect("LoadHeader");
    println!(
        "LoadHeader PASS: elem={} nb={} nrec={} nrel={} shift={} cats={:?} rel={:?}",
        h.elem, h.nb, h.nrec, h.nrel, h.shift, h.cats, h.rel_files
    );
    let bl = check_block_load("zzstreet6.DAT", &data, &h, 0).expect("SetDataBlock");
    println!(
        "SetDataBlock PASS: nE={} letters=\"{}\" validDest={}/{} danger={} root-edges={}",
        bl.ne,
        bl.letters,
        bl.valid_dest_popcount,
        bl.ne,
        bl.danger.len(),
        bl.root_edges.len()
    );
    assert!(bl.danger.is_empty(), "overruns: {:?}", bl.danger);

    // street ids are TEIC order = byte order of uppercase names
    let mut sorted: Vec<&str> = specs.iter().map(|s| s.0).collect();
    sorted.sort_unstable_by_key(|n| n.as_bytes());
    for city in 0..10u32 {
        let list = check_street_list(&data, &rel1, &[(city, city + 1)]).unwrap();
        if !list.is_empty() {
            let names: Vec<&str> = list.iter().map(|&i| sorted[i as usize]).collect();
            println!("city {city}: streets {list:?} {names:?}");
        }
    }

    // ---- crate reader round trip: absolute positions must come back ----
    let nl = lid_format::read(&data).expect("reader");
    let want: Vec<&CityEntry> = {
        let mut v: Vec<&CityEntry> = entries.iter().collect();
        v.sort_unstable_by_key(|c| c.name.as_bytes());
        v
    };
    assert_eq!(nl.elements.len(), want.len());
    for (e, w) in nl.elements.iter().zip(want.iter()) {
        assert_eq!(e.name, w.name, "name order");
        let abs = i64::from(origin.0) + (i64::from(e.x_pau) << h.shift);
        assert!(
            (abs - i64::from(w.lon_pau)).abs() <= (1 << h.shift),
            "lon {}: {abs} vs {}",
            e.name,
            w.lon_pau
        );
    }
    println!(
        "reader round-trip OK ({} names, positions quantized by shift={})",
        nl.elements.len(),
        h.shift
    );

    // ---- HNR (house numbers) GenAttr file + device replay ----
    // street ids: EGZAMINACYJNA=0 PRZYKŁADOWA=1 SIESTA=2 TESTOWA=3 WIDMOWA=4 ZABAWNA=5
    let ranges = [
        HnrRange {
            street_elem: 0,
            odd: (1, 15),
            even: (2, 16),
        },
        HnrRange {
            street_elem: 1,
            odd: (1, 11),
            even: (0, 0),
        },
        HnrRange {
            street_elem: 2,
            odd: (1, 5),
            even: (2, 6),
        },
        HnrRange {
            street_elem: 3,
            odd: (1, 9),
            even: (0, 0),
        },
        HnrRange {
            street_elem: 4,
            odd: (0, 0),
            even: (2, 8),
        },
        HnrRange {
            street_elem: 5,
            odd: (3, 9),
            even: (0, 0),
        },
    ];
    let houter = std::fs::read(format!("{POL}/LID40006.DAT")).unwrap();
    let hnr = build_hnr_file(entries.len() as u32, &ranges, &houter).unwrap();
    std::fs::write("/tmp/opencode/zzhnr40006.DAT", &hnr).unwrap();
    let recs = check_hnr(&hnr).expect("device HNR replay");
    let mut per_street: std::collections::BTreeMap<u32, (u32, u32, u32)> = Default::default();
    for r in &recs {
        let e = per_street.entry(r.street).or_insert((u32::MAX, 0, 0));
        e.0 = e.0.min(r.number);
        e.1 = e.1.max(r.number);
        e.2 += 1;
    }
    for (s, (lo, hi, n)) in &per_street {
        println!(
            "hnr street {s} {}: {n} numbers {lo}..{hi}",
            sorted[*s as usize]
        );
    }
    let want: usize = ranges
        .iter()
        .map(|r| {
            ((if r.odd.1 > 0 {
                (r.odd.1 - r.odd.0) / 2 + 1
            } else {
                0
            }) as usize)
                + ((if r.even.1 > 0 {
                    (r.even.1 - r.even.0) / 2 + 1
                } else {
                    0
                }) as usize)
        })
        .sum();
    assert_eq!(recs.len(), want, "record count");
    println!(
        "HNR device replay OK: {} records, gate/parity/starts consistent",
        recs.len()
    );

    // ---- cross-check: the city browser must be UNAFFECTED by the streets REL00001 variant ----
    let city_lid = std::fs::read("/tmp/opencode/zzcity10.DAT").unwrap();
    let rel0 = std::fs::read("/tmp/opencode/REL00000.DAT").unwrap();
    let rel3 = std::fs::read("/tmp/opencode/REL00003.DAT").unwrap();
    let rel6 = std::fs::read("/tmp/opencode/REL00006.DAT").unwrap();
    let owned: Vec<(u16, &[u8], bool)> = vec![
        (0, &rel0, true),
        (1, &rel1, false),
        (3, &rel3, false),
        (6, &rel6, false),
    ];
    let cl = lid_format::device_sim::check_city_list(&city_lid, &owned, &[(0, 10)]).unwrap();
    assert_eq!(
        cl.list,
        (0..10).collect::<Vec<u32>>(),
        "city list under streets REL00001"
    );
    println!("city list with streets-variant REL00001: all 10 kept (tgt side unchanged) OK");
}
