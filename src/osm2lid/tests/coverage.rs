// Address-coverage integration (LID_format §11.6b / overview §4.4d):
//  * `addr:street` with an INITIAL ("K. Testowej") binds to the real street via token
//    subsequence matching — no duplicate name-list entry is created;
//  * an author string that matches nothing and has no named street within 120 m becomes a
//    stock-style PSEUDO street ("Zgubiona"): its own LID20006 element + real GenAttr rows;
//  * `addr:place` (village addressing) registers under the VILLAGE label and splits into
//    1200 m settlement clusters — like the stock "DEBINY" x12 — one element per cluster.

use std::path::Path;
use std::process::Command;

fn street_numbers(out: &Path) -> (Vec<(u32, String)>, Vec<(u32, Vec<(u32, u32)>)>) {
    // (elements list, per-element house-number `from` values)
    let nl = lid_format::read(&std::fs::read(out.join("LID20006.DAT")).unwrap()).expect("LID");
    let ga_bytes = std::fs::read(out.join("LID40006.DAT")).unwrap();
    let ga = lid_format::read_gen_attr(&ga_bytes).expect("genattr");
    let mut per_street: Vec<(u32, Vec<(u32, u32)>)> = Vec::new();
    for bi in 0..ga.blocks.len() {
        let blk = ga.decode_block(&ga_bytes, bi).expect("block");
        let get = |cc: u32, fl: u32| {
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
        let starts = get(0xc01, 0x8000);
        let nums = get(0xc01, 0);
        let tos = {
            blk.streams
                .iter()
                .find(|s| s.col == 0xc02 && s.flags == 0)
                .map(|s| s.values.clone())
                .unwrap_or_default()
        };
        for (k, &o) in owners.iter().enumerate() {
            let a = starts[k] as usize;
            let b = starts.get(k + 1).copied().unwrap_or(nums.len() as u32) as usize;
            per_street.push((
                blk.elem_start + o,
                nums[a..b].iter().zip(&tos[a..b]).map(|(&n, &t)| (n, t)).collect(),
            ));
        }
    }
    (
        nl.elements
            .iter()
            .enumerate()
            .map(|(i, e)| (i as u32, e.name.clone()))
            .collect(),
        per_street,
    )
}

#[test]
fn coverage_chain_and_pseudo_streets() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let out = Path::new(manifest).join("tests/out_coverage");
    let _ = std::fs::remove_dir_all(&out);
    let inp = format!("{}/tests/data/coverage.osm", manifest);
    let outdir = out.to_str().unwrap().to_string();
    let status = Command::new(env!("CARGO_BIN_EXE_osm2lid"))
        .args([&inp, "-o", &outdir])
        .status()
        .expect("run osm2lid");
    assert!(status.success(), "osm2lid failed");

    let (elems, per_street) = street_numbers(&out);
    let id_of = |nm: &str| elems.iter().find(|(_, n)| n == nm).map(|(i, _)| *i);

    // 1) initials chain: binds the REAL street, and NO "K. Testowej" duplicate exists
    let kt = id_of("Kazimierza Testowej").expect("real street present");
    assert_eq!(id_of("K. Testowej"), None, "initial form must NOT become an entry");
    assert_eq!(
        per_street.iter().find(|(s, _)| *s == kt).map(|(_, v)| v).unwrap(),
        &vec![(12u32, 12)]
    );

    // 2) unmatched string -> pseudo street element + bound numbers
    let zg = id_of("Zgubiona").expect("pseudo-street registered as its own entry");
    assert_eq!(
        per_street.iter().find(|(s, _)| *s == zg).map(|(_, v)| v).unwrap(),
        &vec![(7u32, 7)]
    );

    // 3) addr:place -> village label, one element per 1200 m settlement cluster
    let osada: Vec<u32> = elems.iter().filter(|(_, n)| n == "Osada").map(|(i, _)| *i).collect();
    assert_eq!(osada.len(), 2, "two settlement clusters, two name entries (stock DEBINY shape)");
    let mut recs: Vec<(u32, u32)> = osada
        .iter()
        .flat_map(|id| per_street.iter().find(|(s, _)| s == id).map(|(_, v)| v.clone()).unwrap_or_default())
        .collect();
    recs.sort_unstable();
    assert_eq!(
        recs,
        vec![(3u32, 5), (21, 23)],
        "stock-step merge: consecutive odd singles coalesce per settlement record"
    );

    // 4) SAME label + SAME city: duplicates are separate elements (stock parallel-edge model)
    //    and each cluster's numbers bind to ITS element, never to the first one.
    let chrusty: Vec<u32> = elems.iter().filter(|(_, n)| n == "Chrusty").map(|(i, _)| *i).collect();
    assert_eq!(chrusty.len(), 2, "same-name same-city pseudo duplicates coexist");
    let nums: Vec<Vec<u32>> = chrusty
        .iter()
        .map(|id| {
            let mut v: Vec<u32> = per_street
                .iter()
                .find(|(s, _)| s == id)
                .map(|(_, recs)| recs.iter().map(|(f, _)| *f).collect())
                .unwrap_or_default();
            v.sort_unstable();
            v
        })
        .collect();
    assert!(nums.iter().all(|v| v.len() == 1), "numbers split per duplicate element: {nums:?}");
    let mut all: Vec<u32> = nums.iter().flatten().copied().collect();
    all.sort_unstable();
    assert_eq!(all, vec![4u32, 6]);
}
