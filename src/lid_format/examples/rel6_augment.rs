use lid_format::rel::{get_relations, write_rel, RelIndex};

fn main() {
    let dir = "trials/28 - LID";
    let raw = std::fs::read(format!("{dir}/raw/REL00006.DAT")).unwrap();
    let idx = RelIndex::parse(&raw).unwrap();
    let d = idx.d;
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for k in 0..d[2] as u32 {
        pairs.extend(get_relations(&raw, &idx, true, k, k + 1).unwrap());
    }
    // Modern-variant safety keys (2026-09-26h): the card may seed EITHER label variant of the
    // city's real voivodeship; inherited stock rows sit under historical/asymmetric keys.
    // city idx = 0 BYDGOSZCZ 1 GDANSK 2 KRAKOW 3 LUBLIN 4 POZNAN 5 RADOM 6 SZCZECIN
    //           7 WARSZAWA 8 WROCLAW 9 LODZ   (cities10 order)
    let augment: [(u32, &[u32]); 10] = [
        (0, &[16]),     // KUJAWSKO-POMORSKIE
        (1, &[28]),     // POMORSKIE
        (2, &[25, 27]), // MALOPOLSKIE both variants
        (3, &[23]),     // LUBELSKIE
        (4, &[35]),     // WIELKOPOLSKIE
        (5, &[26]),     // MAZOWIECKIE
        (6, &[18]),     // ZACHODNIOPOMORSKIE
        (7, &[26]),     // MAZOWIECKIE
        (8, &[20, 21]), // DOLNOSLASKIE both variants
        (9, &[19, 22]), // LODZKIE both variants
    ];
    for (city, keys) in augment {
        for &k in keys {
            let p = (k, city);
            if !pairs.contains(&p) {
                pairs.push(p);
            }
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let out = write_rel(u64::from(d[2]), 10, 9, 2, &pairs).unwrap();
    std::fs::write(format!("{dir}/raw/REL00006_augmented.DAT"), &out).unwrap();
    println!("rows={} bytes={}", pairs.len(), out.len());
}
