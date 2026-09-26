//! Integration test: `build_city_relations` against the real stock POL files (read-only
//! ground truth). Verifies the regenerated REL00000/1/3/6 answer the queries the device's
//! city browser makes (`vPopulateCityIndices` district/region ranges + FLI self-relations)
//! with OUR new element ids.
use lid_format::city::{build_city_file, build_city_relations, CityEntry, CityRelations};
use lid_format::rel::{get_relations, RelIndex};

const POL: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                   CRYPTNAV/DATA/DATA/LID/CCP/POL/";

fn stock(name: &str) -> Vec<u8> {
    std::fs::read(format!("{POL}{name}")).expect("read stock")
}

fn cities10() -> Vec<CityEntry> {
    [
        ("BYDGOSZCZ", 18.0084, 53.1235),
        ("GDAŃSK", 18.6464, 54.3520),
        ("KRAKÓW", 19.9450, 50.0647),
        ("LUBLIN", 22.5684, 51.2465),
        ("POZNAŃ", 16.9252, 52.4064),
        ("RADOM", 21.1471, 51.4027),
        ("SZCZECIN", 14.5528, 53.4285),
        ("WARSZAWA", 21.0118, 52.2298),
        ("WROCŁAW", 17.0385, 51.1079),
        ("ŁÓDŹ", 19.4560, 51.7592),
    ]
    .iter()
    .map(|(n, lo, la)| CityEntry::from_deg(n, *lo, *la))
    .collect()
}

/// Element ids in our built file = TEIC order == byte order of uppercase names.
fn idx_of(fam_cities: &[CityEntry], name: &str) -> u32 {
    let mut sorted: Vec<&CityEntry> = fam_cities.iter().collect();
    sorted.sort_unstable_by_key(|c| c.name.as_bytes());
    sorted.iter().position(|c| c.name == name).unwrap() as u32
}

fn q(b: &[u8], by_source: bool, lo: u32, hi: u32) -> Vec<(u32, u32)> {
    let i = RelIndex::parse(b).unwrap();
    get_relations(b, &i, by_source, lo, hi).unwrap()
}

#[test]
fn city_family_queries_answer_with_new_ids() {
    let cs = cities10();
    let fam: CityRelations = build_city_relations(
        &stock("LID20001.DAT"),
        &stock("REL00000.DAT"),
        &stock("REL00001.DAT"),
        &stock("REL00003.DAT"),
        &stock("REL00006.DAT"),
        &cs,
    )
    .expect("build family");

    // District (listID 10) -> cities, exactly like vPopulateCityIndices candidate fetch.
    // stock REL00003 rows: GDAŃSK(199864/199865)->459, WARSZAWA(233759)->457, ŁÓDŹ(240888)->111,
    // BYDGOSZCZ(194288)->348.
    let gdansk = idx_of(&cs, "GDAŃSK");
    let warszawa = idx_of(&cs, "WARSZAWA");
    let lodz = idx_of(&cs, "ŁÓDŹ");
    assert!(q(&fam.rel3, true, 459, 460)
        .iter()
        .any(|&(_, t)| t == gdansk));
    assert!(q(&fam.rel3, true, 457, 458)
        .iter()
        .any(|&(_, t)| t == warszawa));
    assert!(q(&fam.rel3, true, 111, 112).iter().any(|&(_, t)| t == lodz));

    // Homonym guard: village "POZNAŃ" (222146, ~250 km SE) sat in district 156 and village
    // "SZCZECIN" (231069, near Radom) in district 434; neither may list our real cities.
    let poznan = idx_of(&cs, "POZNAŃ");
    let szczecin = idx_of(&cs, "SZCZECIN");
    assert!(
        !q(&fam.rel3, true, 156, 157)
            .iter()
            .any(|&(_, t)| t == poznan),
        "homonym village leaked POZNAŃ into district 156"
    );
    assert!(
        !q(&fam.rel3, true, 434, 435)
            .iter()
            .any(|&(_, t)| t == szczecin),
        "homonym village leaked SZCZECIN into district 434"
    );

    // Region (listID 9) -> cities.
    assert!(q(&fam.rel6, true, 11, 12)
        .iter()
        .any(|&(_, t)| t == idx_of(&cs, "BYDGOSZCZ") || t == idx_of(&cs, "RADOM")));

    // Urban self-relation: our cities with stock urban parts map to (i,i).
    let selfp = q(&fam.rel0, true, warszawa, warszawa + 1);
    assert!(
        selfp.iter().any(|&(_, t)| t == warszawa),
        "expected self-pair for WARSZAWA, got {selfp:?}"
    );

    // Streets: every stock street relation survives, re-owned to nearest of the 10.
    let r1i = RelIndex::parse(&fam.rel1).unwrap();
    assert_eq!(r1i.d[3], 10, "rel1 target count must be our city count");
    let pairs = q(&fam.rel1, true, 0, r1i.d[2]);
    assert!(pairs.len() > 900_000, "street pairs {}", pairs.len());
    assert!(pairs.iter().all(|&(_, t)| t < 10));
    // A Warsaw street must answer WARSZAWA: street 888229 was owned by stock city 233759.
    let owners = q(&fam.rel1, true, 888229, 888230);
    assert!(
        owners.iter().any(|&(_, t)| t == warszawa),
        "street 888229 owners {owners:?}"
    );

    // The city file itself still builds and self-checks with the corrected partner counts.
    let lid = build_city_file(&cs, &stock("LID20001.DAT"));
    lid_format::city::selfcheck(&lid, &stock("LID20001.DAT"), &cs).unwrap();
}

/// End-to-end device replica: vPopulateCityIndices candidate collection + gates over the
/// generated family must list every city, with no past-buffer stream reads.
#[test]
fn city_list_simulator_lists_all_new_cities() {
    use lid_format::device_sim::check_city_list;
    let cs = cities10();
    let lid = build_city_file(&cs, &stock("LID20001.DAT"));
    let fam: CityRelations = build_city_relations(
        &stock("LID20001.DAT"),
        &stock("REL00000.DAT"),
        &stock("REL00001.DAT"),
        &stock("REL00003.DAT"),
        &stock("REL00006.DAT"),
        &cs,
    )
    .expect("build family");
    let rels: [(&[u8], bool); 4] = [
        (&fam.rel0, true),
        (&fam.rel1, false),
        (&fam.rel3, false),
        (&fam.rel6, false),
    ];
    let owned: Vec<(u16, &[u8], bool)> = rels
        .iter()
        .enumerate()
        .map(|(i, (b, bs))| (i as u16, *b, *bs))
        .collect();
    let r = check_city_list(&lid, &owned, &[(0, cs.len() as u32)]).expect("list");
    assert!(r.danger.is_empty(), "past-buffer reads: {:?}", r.danger);
    assert_eq!(r.list, (0..cs.len() as u32).collect::<Vec<u32>>());
    assert!(r.dropped.is_empty(), "dropped: {:?}", r.dropped);
}
