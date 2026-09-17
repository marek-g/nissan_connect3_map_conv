// osm2lid crossing integration: run the bin on a two-streets-crossing fixture, then validate
// LID30006.DAT with `lid_format::read_crossings` (the same readers that decode stock DEU
// `LID30006`). Element domain must equal the street name-list element count; the two crossing
// streets list each other, the isolated third street lists nothing.

use std::path::Path;
use std::process::Command;

#[test]
fn crossings_osm_roundtrip() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let out = Path::new(manifest).join("tests/out_crossings");
    let _ = std::fs::remove_dir_all(&out);
    let inp = format!("{}/tests/data/cross.osm", manifest);
    let outdir = out.to_str().unwrap().to_string();
    let status = Command::new(env!("CARGO_BIN_EXE_osm2lid"))
        .args([&inp, "-o", &outdir])
        .status()
        .expect("run osm2lid");
    assert!(status.success(), "osm2lid failed");

    let names_bytes = std::fs::read(out.join("LID20006.DAT")).unwrap();
    let nl = lid_format::read(&names_bytes).expect("street LID");
    let id_of = |n: &str| {
        nl.elements
            .iter()
            .position(|e| e.name == n)
            .expect("street element") as u32
    };
    let lipowa = id_of("Aleja Lipowa");
    let polna = id_of("Polna");
    let samotna = id_of("Samotna");

    let b = std::fs::read(out.join("LID30006.DAT")).unwrap();
    assert!(
        lid_format::is_crossing(&b),
        "generated file must be crossing kind"
    );
    assert!(
        !lid_format::is_name_list(&b),
        "crossing must not be a name-list"
    );
    let ci = lid_format::read_crossings(&b).expect("read_crossings");
    assert_eq!(
        ci.element_count as usize,
        nl.elements.len(),
        "element domain = street list domain"
    );

    let mut all: Vec<(u32, Vec<u32>)> = Vec::new();
    for bi in 0..ci.blocks.len() {
        let cs = ci.decode_crossings(&b, bi).expect("decode block");
        for c in &cs {
            assert_ne!(c.streets.first(), Some(&c.element), "self never listed");
        }
        all.extend(cs.into_iter().map(|c| (c.element, c.streets)));
    }
    assert_eq!(
        all,
        vec![(lipowa, vec![polna]), (polna, vec![lipowa])],
        "each street meets the other at the shared node (both onecell ends)"
    );
    assert!(!all.iter().any(|(e, _)| *e == samotna));
}
