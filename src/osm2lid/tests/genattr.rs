// osm2lid GenAttr integration: run the bin on a tiny OSM XML fixture, then validate LID40006.DAT
// end-to-end with the `lid_format` readers (same readers that decode the author's `POL/LID40006`).
// The writer chunks the 3 records into 2 blocks (a GenAttr file needs >= 2 TOC blocks).

use std::path::Path;
use std::process::Command;

#[test]
fn genattr_osm_roundtrip() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let out = Path::new(manifest).join("tests/out_genattr");
    let _ = std::fs::remove_dir_all(&out);
    let inp = format!("{}/tests/data/addrs.osm", manifest);
    let outdir = out.to_str().unwrap().to_string();
    let status = Command::new(env!("CARGO_BIN_EXE_osm2lid"))
        .args([&inp, "-o", &outdir])
        .status()
        .expect("run osm2lid");
    assert!(status.success(), "osm2lid failed");

    // 1. street name-list carries both streets; GenAttr joins through *this* element order.
    let nl = lid_format::read(&std::fs::read(out.join("LID20006.DAT")).unwrap()).expect("street LID");
    let names: Vec<&str> = nl.elements.iter().map(|e| e.name.as_str()).collect();
    let marsz = names.iter().position(|n| *n == "Marszalkowska").expect("Marszalkowska element") as u32;
    let nowo = names.iter().position(|n| *n == "Nowogrodzka").expect("Nowogrodzka element") as u32;

    // 2. GenAttr: 3 numeric housenumbers (11A dropped — stock number column is u32), chunks to >=2 blocks.
    let ga_bytes = std::fs::read(out.join("LID40006.DAT")).unwrap();
    let ga = lid_format::read_gen_attr(&ga_bytes).expect("not a GenAttr file");
    assert_eq!(ga.element_count, 3);
    assert!(ga.blocks.len() >= 2);

    let mut st_cols = Vec::new();   // 0xc01 8000: per-street addr counts, blocks concatenated
    let mut st_vals = Vec::new();   // 0xc01 0000: addr elem ids
    let mut to_street = Vec::new(); // 0x002 8000: elem -> street id
    let mut number = Vec::new();    // 0x00c 8000: elem -> housenumber
    let mut num_to = Vec::new();    // 0x00c 0000
    let mut parity = Vec::new();    // 0xc0a 4000: elem -> even
    for bi in 0..ga.blocks.len() {
        let blk = ga.decode_block(&ga_bytes, bi).expect("block decode");
        let (a, c) = (ga.blocks[bi].elem_start, ga.blocks[bi].elem_end);
        let col = |cc: u32, fl: u32| -> Vec<u32> {
            blk.streams.iter().find(|s| s.col == cc && s.flags == fl)
                .unwrap_or_else(|| panic!("no col {cc:#x}/{fl:#x} in block {bi}")).values.clone()
        };
        let bits = |cc: u32, fl: u32| -> Vec<bool> {
            blk.streams.iter().find(|s| s.col == cc && s.flags == fl)
                .unwrap_or_else(|| panic!("no bits col {cc:#x}/{fl:#x} in block {bi}")).bits.clone()
        };
        assert_eq!(col(0xc01, 0x0000).len(), col(0xc01, 0x8000).iter().sum::<u32>() as usize, "addr vals = Σ counts");
        assert_eq!(col(0x00c, 0x0000).len() as u32, c - a + 1, "number tos = block elems");
        st_cols.extend(col(0xc01, 0x8000));
        st_vals.extend(col(0xc01, 0x0000));
        to_street.extend(col(0x002, 0x8000));
        number.extend(col(0x00c, 0x8000));
        num_to.extend(col(0x00c, 0x0000));
        parity.extend(bits(0xc0a, 0x4000));
    }

    // street-domain is the 2 address-bearing streets, ascending street-id order in every block:
    let (lo, hi) = (marsz.min(nowo), marsz.max(nowo)); // ids of the two streets
    assert_eq!(st_cols.len(), 2, "one (count) entry per address-bearing street");
    // the two Marszalkowska addrs come first (lowest elem ids 0,1), Nowogrodzka's elem 2 last.
    assert_eq!(st_vals, vec![0u32, 1, 2]);
    let first_street_n = st_cols[0];
    assert_eq!(first_street_n, if marsz < nowo { 2 } else { 1 });
    assert_eq!(st_cols[1], if marsz < nowo { 1 } else { 2 });
    // street->addr membership: Marszalkowska elems {0,1}; Nowogrodzka elem {2}
    let marsz_addrs = if marsz < nowo { &st_vals[..first_street_n as usize] } else { &st_vals[1..] };
    assert_eq!(marsz_addrs, &[0u32, 1u32]);

    assert_eq!(to_street, vec![marsz, marsz, nowo], "addr->street ids match LID20006 element order");
    assert_eq!(number, vec![10, 11, 5], "housenumber froms");
    assert_eq!(num_to, vec![10, 11, 5], "housenumber tos (== number)");
    assert_eq!(parity, vec![true, false, false], "even-number parity bits (10 even; 11,5 odd)");
    let _ = (hi, lo);

    std::fs::remove_dir_all(&out).ok();
}
