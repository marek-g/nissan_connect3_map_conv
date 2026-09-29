// Test variant: MAX-COVERAGE REL00003 (district->city) and REL00006 (region->city) so that EVERY
// source key maps to ALL of our cities. Removes the "seeded region not covered" variable so a card
// test cleanly discriminates (LID loads + REL->city populates) vs (LID/NL-init fails).
use lid_format::rel::{write_rel, RelIndex};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let outdir = args.get(1).cloned().unwrap_or("/tmp/opencode".into());
    let pol = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
               CRYPTNAV/DATA/DATA/LID/CCP/POL/";
    let n_cities: u32 = args.get(2).map(|s| s.parse().unwrap()).unwrap_or(10);

    for (name, list_a) in [("REL00003", 10u16), ("REL00006", 9u16)] {
        let stock = std::fs::read(format!("{pol}{name}.DAT")).expect("read stock rel");
        let n_src = RelIndex::parse(&stock).unwrap().d[2];
        let rels: Vec<(u32, u32)> = (0..n_src)
            .flat_map(|s| (0..n_cities).map(move |c| (s, c)))
            .collect();
        let bytes = write_rel(n_src as u64, n_cities as u64, list_a, 2, &rels).expect("write_rel");
        let path = format!("{outdir}/{name}_maxcov.DAT");
        std::fs::write(&path, &bytes).expect("write");
        println!(
            "{name}: src={} tgt={} listA={} pairs={} -> {} bytes",
            n_src,
            n_cities,
            list_a,
            rels.len(),
            bytes.len()
        );
    }
}
