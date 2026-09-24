// osm2lid point-address integration: the PA_20006 sidecar must carry one access point per street
// element, positioned so that (street name-list anchor + PA position) lands on the street's lowest
// numeric address — the exact sum the device computes in `bGetPACells` (§11.7b). Card test pending.

use lid_format::pa::{decode_pa_detail_block, parse_pa, PaIndex};
use std::path::Path;
use std::process::Command;

#[test]
fn pa_osm_roundtrip() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let out = Path::new(manifest).join("tests/out_pa");
    let _ = std::fs::remove_dir_all(&out);
    let inp = format!("{}/tests/data/addrs.osm", manifest);
    let outdir = out.to_str().unwrap().to_string();
    let status = Command::new(env!("CARGO_BIN_EXE_osm2lid"))
        .args([&inp, "-o", &outdir])
        .status()
        .expect("run osm2lid");
    assert!(status.success());

    let raw = std::fs::read(out.join("PA_20006.DAT")).unwrap();
    assert_eq!(
        &raw[..8],
        &[2, 4, 3, 0, 0, 0, 0xEC, 0x41],
        "PA identity = the street list (no +20000)"
    );
    assert_eq!(&raw[0x0c..0x10], &[5, 0, 0, 0], "file kind = PA");

    let nl =
        lid_format::read(&std::fs::read(out.join("LID20006.DAT")).unwrap()).expect("street LID");
    let idx = parse_pa(&raw).expect("PA parses");
    assert_eq!(
        idx.domain as usize,
        nl.elements.len(),
        "domain = street elems"
    );
    let dl = idx.detail_list().expect("DETAIL list");

    // decode every element through the device walk (locate + decode, cached per block).
    let mut entries: Vec<lid_format::pa::PaDetailEntry> = Vec::new();
    let mut cache: Option<(usize, _)> = None;
    for e in 0..idx.domain {
        let (off, size, start, width) = PaIndex::locate(dl, e).expect("locate");
        if cache.as_ref().map_or(true, |c| c.0 != off) {
            cache = Some((off, decode_pa_detail_block(&raw, off, size).expect("block")));
        }
        let blk = &cache.as_ref().unwrap().1;
        assert!((start..start + width).contains(&e));
        entries.push(blk.entries[(e - start) as usize].clone());
    }
    assert_eq!(entries.len(), nl.elements.len());
    assert!(
        entries
            .iter()
            .all(|e| e.cell == 0 && e.ratio == 100 && !e.left && !e.right),
        "neutral cell/side, ratio 100"
    );

    // positions: anchor(abs) + rel = the lowest numeric address of the street.
    let PAU: f64 = (1i64 << 31) as f64 / 180.0;
    let p = |d: f64| (d * PAU) as i64;
    let p2 = |a: f64, b: f64| (p(a) + p(b)) / 2; // writer's centroid (int PAU, floor)
    for (name, ax, ay, hxr, hyr) in [
        // Marszalkowska centroid (100..101), addr 10 = node 1; anchor city Ceszin.
        (
            "Marszalkowska",
            (21.0000, 21.0010),
            (52.0000, 52.0010),
            21.0005,
            52.0005,
        ),
        // Nowogrodzka centroid (102..103), addr 5 = node 4 (addr:place join).
        (
            "Nowogrodzka",
            (21.0100, 21.0110),
            (52.0100, 52.0110),
            21.0105,
            52.0105,
        ),
    ] {
        let (ei, ne) = nl
            .elements
            .iter()
            .enumerate()
            .find(|(_, e)| e.name == name)
            .expect("street elem");
        let (cx, cy) = match name {
            "Marszalkowska" => (p(21.00060), p(51.99950)),
            _ => (p(21.00940), p(52.01080)),
        };
        // stored name-list delta + its city anchor = the street's absolute anchor.
        let anchor = (cx + i64::from(ne.x_pau), cy + i64::from(ne.y_pau));
        assert_eq!(
            anchor,
            (p2(ax.0, ax.1), p2(ay.0, ay.1)),
            "{name} anchor math"
        );
        let rel = entries[ei].pos.expect("PA pos");
        assert_eq!(
            (anchor.0 + i64::from(rel.0), anchor.1 + i64::from(rel.1)),
            (p(hxr), p(hyr)),
            "{name}: anchor + PA pos must hit the address node"
        );
    }

    std::fs::remove_dir_all(&out).ok();
}
