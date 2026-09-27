use lid_format::device_sim::*;
use lid_format::rel::{get_relations, RelIndex};
use std::collections::BTreeSet;

fn run(nm: &str, lid: &[u8], rels: &[(u16, &[u8], bool)], entries: &[u32]) {
    match check_city_list_entries(lid, rels, entries) {
        Ok(cl) => println!(
            "{nm}: ne={} keys={} candidates={} list={} dropped={} danger={}",
            cl.ne,
            cl.keys,
            cl.candidates,
            cl.list.len(),
            cl.dropped.len(),
            cl.danger.len()
        ),
        Err(e) => println!("{nm}: GATE {e:?}"),
    }
}

fn main() {
    let fw = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";
    let ours = std::fs::read("trials/28 - LID/raw/LID20001.DAT").unwrap();
    let stock = std::fs::read("/tmp/opencode/stock1r.DAT").unwrap();
    let s0 = std::fs::read(format!("{fw}REL00000.DAT")).unwrap();
    let s1 = std::fs::read(format!("{fw}REL00001.DAT")).unwrap();
    let s3 = std::fs::read(format!("{fw}REL00003.DAT")).unwrap();
    let s6 = std::fs::read(format!("{fw}REL00006.DAT")).unwrap();
    let stockrels: Vec<(u16, &[u8], bool)> = vec![
        (0, &s0, true),
        (1, &s1, false),
        (3, &s3, false),
        (6, &s6, false),
    ];
    let o = "trials/28 - LID/raw";
    let o1 = std::fs::read(format!("{o}/REL00001.DAT")).unwrap();
    let o3 = std::fs::read(format!("{o}/REL00003.DAT")).unwrap();
    let o6 = std::fs::read(format!("{o}/REL00006.DAT")).unwrap();
    let ourrels: Vec<(u16, &[u8], bool)> = vec![(1, &o1, false), (3, &o3, false), (6, &o6, false)];
    // Region-DB entry approximation: every city id that stock REL content files key rows for
    // (the stock POL region entries reference exactly these stock city elements).
    let mut entries: BTreeSet<u32> = BTreeSet::new();
    for (id, b) in [(1u16, &s1[..]), (3, &s3[..]), (6, &s6[..])] {
        let idx = RelIndex::parse(b).unwrap();
        for (k, _) in get_relations(b, &idx, false, 0, idx.d[3]).unwrap() {
            entries.insert(k);
        }
        let _ = id;
    }
    let entries: Vec<u32> = entries.into_iter().collect();
    println!("entries(region DB city ids) = {}", entries.len());
    run("stockLID+stockREL", &stock, &stockrels, &entries);
    run("V1:  ourLID +stockREL", &ours, &stockrels, &entries);
    run("2b:  ourLID +  ourREL", &ours, &ourrels, &entries);
    // Variant "ID-merge" preview: our file but entries restricted to ids we actually hold.
    let fake: Vec<u32> = (0..10u32).collect();
    run(
        "hyp: ourLID +  ourREL + entries(0..9)",
        &ours,
        &ourrels,
        &fake,
    );

    // REGION browse (CONFIRMED 2026-09-26h): LID20005 = 38 voivodeship elements (DE/ASCII/
    // diacritic variants); selecting one seeds descriptor ResumingElementIndices from REL00006
    // (src = LID20005 id -> city ids) -> entries via vAdd100PercentMatches -> bGoToElemet gate.
    // Scenario "malopolskie": keys 25 (MALOPOLSKIE) + 26 (MAZOWIECKIE, Krakow's stock member)
    // + 27 (MALOPOLSKIE-diacritic). Scenario "all-38": whole country set.
    for (nm, keys) in [
        ("malopolskie {25,26,27}", vec![25u32, 26, 27]),
        ("all-38", (0..38u32).collect::<Vec<u32>>()),
    ] {
        for (sc, lid, rel) in [
            ("stockLID+stockREL", &stock[..], &s6[..]),
            ("V1  ourLID+stockREL", &ours[..], &s6[..]),
            ("2b  ourLID+ ourREL", &ours[..], &o6[..]),
        ] {
            match check_region_browse(lid, rel, &keys) {
                Ok(cl) => println!(
                    "region[{nm}] {sc}: ne={} targets={} listed={} dropped={} -> {:?}",
                    cl.ne,
                    cl.keys,
                    cl.list.len(),
                    cl.dropped.len(),
                    cl.list
                ),
                Err(e) => println!("region[{nm}] {sc}: GATE {e:?}"),
            }
        }
    }
}
