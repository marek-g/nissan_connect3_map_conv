// PA <-> GenAttr cell join integration. A PA detail cell id is passed verbatim by
// `bGetPACellIDs 00b898dc` to `NLGenAttrProcessor::bGetCellOfBlock` -> the street block's
// `NLCellIdAttrVector::enGetCells`, so it must be the block cell-table row ordinal of the
// street's lowest-numbered record — not any global id. Fixture: one street, two segments; the
// lowest number (11) sits on the SECOND segment of the build -> the PA cell must be row 1.

use std::path::Path;
use std::process::Command;

#[test]
fn pa_cells_point_at_the_lowest_records_row() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let out = Path::new(manifest).join("tests/out_pa_cells");
    let _ = std::fs::remove_dir_all(&out);
    let inp = format!("{}/tests/data/addrs2.osm", manifest);
    let outdir = out.to_str().unwrap().to_string();
    let status = Command::new(env!("CARGO_BIN_EXE_osm2lid"))
        .args([&inp, "-o", &outdir])
        .status()
        .expect("run osm2lid");
    assert!(status.success(), "osm2lid failed");

    // The one address-bearing street element.
    let nl =
        lid_format::read(&std::fs::read(out.join("LID20006.DAT")).unwrap()).expect("street LID");
    let duza = nl
        .elements
        .iter()
        .position(|e| e.name == "Duza")
        .expect("Duza element") as u32;

    // GenAttr: the street's record numbers, owner order (0xc11 now carries the OWNER city id,
    // not the table row — the PA cell id is the block table row assigned at build time).
    let ga_bytes = std::fs::read(out.join("LID40006.DAT")).unwrap();
    let ga = lid_format::read_gen_attr(&ga_bytes).expect("genattr index");
    let mut nums_all: Vec<u32> = Vec::new(); // our street's house numbers
    let mut nrows = 0usize;
    for bi in 0..ga.blocks.len() {
        let blk = ga.decode_block(&ga_bytes, bi).expect("block");
        let stream = |cc: u32, fl: u32| {
            blk.streams
                .iter()
                .find(|s| s.col == cc && s.flags == fl)
                .map(|s| s.values.clone())
                .unwrap_or_default()
        };
        let bits = blk
            .streams
            .iter()
            .find(|s| s.col == 0xc01 && s.flags == 0x4000)
            .map(|s| s.bits.clone())
            .unwrap_or_default();
        let owners: Vec<u32> = (0..bits.len() as u32).filter(|&i| bits[i as usize]).collect();
        let starts = stream(0xc01, 0x8000);
        let nums = stream(0xc01, 0);
        nrows += stream(0x0004, 0).len();
        for (k, &o) in owners.iter().enumerate() {
            if blk.elem_start + o != duza {
                continue;
            }
            let a = starts[k] as usize;
            let b = starts.get(k + 1).copied().unwrap_or(nums.len() as u32) as usize;
            for r in a..b.min(nums.len()).max(a) {
                nums_all.push(nums[r]);
            }
        }
    }
    assert_eq!(nums_all.len(), 2, "one even + one odd consolidated record");
    assert!(nums_all.contains(&11), "lowest number 11 present");
    assert_eq!(nrows, 2, "fixture street spans two table rows");

    // PA detail: the street's access point cites exactly that row.
    let pa_bytes = std::fs::read(out.join("PA_20006.DAT")).expect("PA file");
    let idx = lid_format::pa::parse_pa(&pa_bytes).expect("pa index");
    let dl = idx.detail_list().expect("detail list");
    let (off, size, start, _w) = lid_format::pa::PaIndex::locate(dl, duza).expect("locate");
    let blk = lid_format::pa::decode_pa_detail_block(&pa_bytes, off, size).expect("decode");
    let det = &blk.entries[(duza - start) as usize];
    assert!(det.pos.is_some(), "access point position present");
    assert_eq!(det.cell, 1, "PA cell = table row of the lowest record's segment");
    assert_ne!(det.cell, 0, "fixture's lowest number sits on the second segment");

    std::fs::remove_dir_all(&out).ok();
}
