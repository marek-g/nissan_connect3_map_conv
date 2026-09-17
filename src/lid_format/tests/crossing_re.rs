//! RE/oracle test for the crossing (`LID3%05u`, fileID+10000) reader.
//! Run: CROSS_FILE=.../DEU/LID30006.DAT CROSS_NAMES=.../DEU/LID20006.DAT \
//!      cargo test --manifest-path src/lid_format/Cargo.toml --test crossing_re -- \
//!      --ignored --nocapture
use lid_format::{read, read_crossings};

#[test]
#[ignore = "oracle on stock DEU card files"]
fn crossings_decode_and_name_resolve() {
    let cross_file = std::env::var("CROSS_FILE").unwrap();
    let names_file = std::env::var("CROSS_NAMES").unwrap();
    let b = std::fs::read(&cross_file).unwrap();
    let names = std::fs::read(&names_file).unwrap();
    let nl = read(&names).expect("street name list read");
    assert_eq!(
        nl.elements.len() as u32,
        read_crossings(&b).unwrap().element_count
    );
    let ci = read_crossings(&b).unwrap();
    let mut total = 0usize;
    let mut pairs_shown = 0usize;
    let mut multi = 0usize;
    for bi in 0..ci.blocks.len() {
        let cs = ci
            .decode_crossings(&b, bi)
            .unwrap_or_else(|e| panic!("block {bi}: {e}"));
        for c in &cs {
            assert!(c.element < ci.element_count);
            if c.streets.len() > 1 {
                multi += 1;
            }
            for sid in &c.streets {
                assert!(*sid < nl.elements.len() as u32, "street id {sid} OOB");
            }
            if pairs_shown < 6 {
                let pair: Vec<&str> = c
                    .streets
                    .iter()
                    .take(6)
                    .map(|&sid| nl.elements[sid as usize].name.as_str())
                    .collect();
                println!("crossing elem {} -> {pair:?}", c.element);
                pairs_shown += 1;
            }
        }
        total += cs.len();
    }
    println!(
        "blocks={} crossings={total} multiway={multi} street_domain={}",
        ci.blocks.len(),
        nl.elements.len()
    );
    assert!(total > 1_000_000, "DEU should expose millions of crossings");
}
