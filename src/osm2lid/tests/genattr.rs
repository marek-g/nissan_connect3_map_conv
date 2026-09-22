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
    let nl =
        lid_format::read(&std::fs::read(out.join("LID20006.DAT")).unwrap()).expect("street LID");
    let names: Vec<&str> = nl.elements.iter().map(|e| e.name.as_str()).collect();
    let marsz = names
        .iter()
        .position(|n| *n == "Marszalkowska")
        .expect("Marszalkowska element") as u32;
    let nowo = names
        .iter()
        .position(|n| *n == "Nowogrodzka")
        .expect("Nowogrodzka element") as u32;

    // 1b. §12.5: street coordinates are stored relative to the street's city (nearest place node),
    // one city per block, with the file-level anchor advertised. Ceszin anchors Marszalkowska,
    // Bemow anchors Nowogrodzka (distance from the way centroids).
    const PAU: f64 = (1i64 << 31) as f64 / 180.0;
    let p = |d: f64| (d * PAU) as i32;
    let p2 = |a: f64, b: f64| (((a * PAU) as i64 + (b * PAU) as i64) / 2) as i32; // the writer's centroid
    assert!(
        nl.origin.is_some(),
        "street tile must advertise a file-level position anchor"
    );
    assert!(
        nl.block_count >= 2,
        "the two cities must land in separate blocks, got {}",
        nl.block_count
    );
    for (name, cx, cy, ax, ay) in [
        (
            "Marszalkowska",
            (21.0000, 21.0010),
            (52.0000, 52.0010),
            21.00060,
            51.99950,
        ),
        (
            "Nowogrodzka",
            (21.0100, 21.0110),
            (52.0100, 52.0110),
            21.00940,
            52.01080,
        ),
    ] {
        let e = nl.elements.iter().find(|e| e.name == name).unwrap();
        assert!(e.has_pos, "{name} lost its position");
        assert_eq!(
            (e.x_pau, e.y_pau),
            (p2(cx.0, cx.1) - p(ax), p2(cy.0, cy.1) - p(ay)),
            "{name} city-relative position"
        );
    }

    // 1c. the settlement gazetteer stays coordinate-free (stock settlement-list flavor, listID 2 =
    // LID20001; note stock LID20000 is the listID-129 HNR domain, §11 inventory).
    let cities =
        lid_format::read(&std::fs::read(out.join("LID20001.DAT")).unwrap()).expect("city LID");
    assert_eq!(
        cities.origin, None,
        "gazetteer carries no origin (stock -1/-1)"
    );
    assert!(
        cities.elements.iter().all(|e| !e.has_pos),
        "gazetteer carries no coordinates"
    );

    // 2. GenAttr in the STOCK device layout (§11.6b): element domain = the street name-list;
    // 0xc01 = per-street house numbers, 0xc09/0xc0a = per-record parity, 0xc11 = record gate,
    // 0xc03.. = empty string lists. 3 numeric housenumbers (11A dropped — u32 column).
    let ga_bytes = std::fs::read(out.join("LID40006.DAT")).unwrap();
    let ga = lid_format::read_gen_attr(&ga_bytes).expect("not a GenAttr file");
    assert_eq!(
        ga.element_count,
        nl.elements.len() as u32,
        "GenAttr domain = street elements"
    );
    assert!(ga.blocks.len() >= 2);

    // replay enGetHnrIndices/enGetHnr across all blocks: street -> house numbers.
    let mut street_nums: Vec<(u32, Vec<u32>)> = Vec::new();
    let mut parity_even: Vec<bool> = Vec::new(); // 0xc09 (even), records in owner order
    let mut tos: Vec<u32> = Vec::new(); // 0xc02 per-record `to` bound (NLHnr+0x0c); single = number
    let mut ids: Vec<u32> = Vec::new(); // 0x001 per-street owner city ids
    for bi in 0..ga.blocks.len() {
        let blk = ga.decode_block(&ga_bytes, bi).expect("block decode");
        let (a, c) = (ga.blocks[bi].elem_start, ga.blocks[bi].elem_end);
        let stream = |cc: u32, fl: u32| {
            blk.streams
                .iter()
                .find(|s| s.col == cc && s.flags == fl)
                .unwrap_or_else(|| panic!("no col {cc:#x}/{fl:#x} in block {bi}"))
        };
        let ex = stream(0xc01, 0x4000).bits.clone();
        assert_eq!(ex.len(), (c - a + 1) as usize, "owner domain = block width");
        let offs = stream(0xc01, 0x8000).values.clone();
        let vals = stream(0xc01, 0x0000).values.clone();
        assert_eq!(
            stream(0xc11, 0x4000).param as usize,
            vals.len(),
            "enGetHnr gate = record count"
        );
        assert_eq!(stream(0xc09, 0).bits.len(), vals.len(), "parity per record");
        assert_eq!(stream(0xc02, 0).values.len(), vals.len(), "to bound per record");
        assert_eq!(stream(0xc03, 0x4000).param as usize, vals.len());
        assert!(stream(0xc03, 0x4000).bits.iter().all(|&b| !b));
        tos.extend(stream(0xc02, 0).values.iter().copied());
        parity_even.extend(stream(0xc09, 0).bits.iter().copied());
        for (k, o) in (0..ex.len()).filter(|&i| ex[i]).enumerate() {
            let start = offs[k] as usize;
            let end = offs.get(k + 1).copied().unwrap_or(vals.len() as u32) as usize;
            street_nums.push((a + o as u32, vals[start..end].to_vec()));
        }
        // 0xc11 = per-record OWNING STREET element id (stock: block-8 values 957, 20021..20029 are
        // street-list ids; bHasValidOwner uses it as the street-set key for the city's HNR list).
        let c11 = stream(0xc11, 0).values.clone();
        assert!(!c11.is_empty(), "0xc11 per record");
        assert!(
            c11.iter().all(|&v| v < 2 || v == 0xffff_ffff),
            "0xc11 = owning street id"
        );
        assert_eq!(
            stream(0x0004, 0).values,
            vec![1u32],
            "one table row per street segment, RNW cluster id 1"
        );
        assert!(stream(0x0005, 0).bits.iter().all(|&b| b));
        ids.extend(stream(0x0001, 0x0000).values.iter().copied());
    }
    // 0x001: per-street OWNER city id lists (both fixture cities own each street; the reported
    // city leads, the 3 km neighbor follows — city id order = city_ids insertion per street).
    assert_eq!(ids, vec![0u32, 1, 1, 0], "0x001 = per-street owner city ids");
    assert_eq!(street_nums.len(), 2, "two address-bearing streets");
    let nums_of = |sid: u32| {
        street_nums
            .iter()
            .find(|(s, _)| *s == sid)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let (marsz_n, nowo_n) = (nums_of(marsz), nums_of(nowo));
    assert_eq!(marsz_n, vec![10u32, 11], "Marszalkowska numbers");
    assert_eq!(nowo_n, vec![5u32], "Nowogrodzka number");
    // per-record parity & `to` bound follow owner (street ascending) order; single numbers have to == from:
    let (exp_parity, exp_tos) = if marsz < nowo {
        (vec![true, false, false], vec![10u32, 11, 5]) // to == the number itself
    } else {
        (vec![false, false, true], vec![5u32, 10, 11])
    };
    assert_eq!(parity_even, exp_parity, "0xc09 even bits");
    assert_eq!(tos, exp_tos, "0xc02 to bound = the number");

    // 3. outer-header identities (the device binds list files by these first bytes) + the REL matrix.
    for (f, id, kind) in [
        ("LID20001.DAT", 2u8, 1u8),
        ("LID20006.DAT", 3, 1),
        ("LID40006.DAT", 3, 3),
    ] {
        let raw = std::fs::read(out.join(f)).unwrap();
        assert_eq!(
            &raw[..8],
            &[2, 4, id, 0, 0, 0, 0xEC, 0x41],
            "{f} rIdxListID identity"
        );
        assert_eq!(&raw[0x0c..0x10], &[kind, 0, 0, 0], "{f} file kind");
    }
    let rel = std::fs::read(out.join("REL00001.DAT")).unwrap();
    let ridx = lid_format::rel::RelIndex::parse(&rel).expect("REL00001 parses");
    assert_eq!(
        (ridx.d[0], ridx.d[1]),
        (3, 2),
        "file stores (street, city) list ids"
    );
    let (street_marsz, street_nowo) = (marsz, nowo);
    let mut pairs = lid_format::rel::get_relations(&rel, &ridx, true, 0, 2).unwrap();
    pairs.sort_unstable();
    // stock multiple-city shape (CHECKED on stock_CCP_POL.db): each street pairs with EVERY city
    // within 3 km of its position — here both fixture cities are within range of both streets
    let mut want: Vec<(u32, u32)> = vec![];
    for s in [street_marsz, street_nowo] {
        for t in 0..2u32 {
            want.push((s, t));
        }
    }
    want.sort_unstable();
    assert_eq!(pairs, want, "street->city pairs = every city within 3 km (stock shape)");
    assert!(pairs.iter().all(|&(_, t)| t < 2), "city elems in range");

    std::fs::remove_dir_all(&out).ok();
}
