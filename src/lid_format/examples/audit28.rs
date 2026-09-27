use lid_format::device_sim::*;
use lid_format::rel::RelIndex;

fn audit_lid(path: &str, tag: &str) {
    let b = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            println!("{tag}: READ ERR {e}");
            return;
        }
    };
    match check_name_list_header(tag, &b) {
        Err(e) => println!("{tag}: HEADER GATE {e:?}"),
        Ok(h) => {
            println!(
                "{tag}: hdr elem={} nb={} nrec={} nrel={} rel={:?} cats={:?}",
                h.elem, h.nb, h.nrec, h.nrel, h.rel_files, h.cats
            );
            for bi in 0..h.nb as usize {
                match check_block_load(tag, &b, &h, bi) {
                    Err(e) => println!("  blk{bi}: GATE {e:?}"),
                    Ok(bl) => {
                        if bl.letters.is_empty() || bi < 2 {
                            println!(
                                "  blk{bi}: edges={} letters='{}' valid_pop={} danger={}",
                                bl.root_edges.len(),
                                bl.letters,
                                bl.valid_dest_popcount,
                                bl.danger.len()
                            );
                        } else {
                            println!(
                                "  blk{bi}: edges={} valid_pop={} danger={} letters='#{}'",
                                bl.root_edges.len(),
                                bl.valid_dest_popcount,
                                bl.danger.len(),
                                bl.letters.chars().count()
                            );
                        }
                        for d in &bl.danger {
                            println!("    DANGER {d}");
                        }
                    }
                }
            }
        }
    }
}

fn main() {
    let base = "trials/28 - LID/raw/";
    for f in ["LID20001.DAT", "LID20006.DAT", "LID40006.DAT"] {
        audit_lid(&format!("{base}{f}"), f);
    }
    println!("--- stock LID20001 reference ---");
    audit_lid("/tmp/opencode/stock1r.DAT", "STOCK-20001");
    println!("--- REL dims (ours) ---");
    for f in [
        "REL00000.DAT",
        "REL00001.DAT",
        "REL00003.DAT",
        "REL00006.DAT",
    ] {
        let b = std::fs::read(format!("{base}{f}")).unwrap();
        match RelIndex::parse(&b) {
            Err(e) => println!("{f}: GATE {e}"),
            Ok(ix) => println!("{f}: d={:?}", &ix.d[..8]),
        }
    }
    println!("--- HNR ---");
    let b = std::fs::read(format!("{base}LID40006.DAT")).unwrap();
    match check_hnr(&b) {
        Err(e) => println!("LID40006: GATE {e:?}"),
        Ok(v) => println!(
            "LID40006: {} hnr recs, first {:?} last {:?}",
            v.len(),
            v.first(),
            v.last()
        ),
    }
    println!("--- city list V2b prediction (our LID + our RELs, ctx [0..10)) ---");
    let lid = std::fs::read(format!("{base}LID20001.DAT")).unwrap();
    let r: Vec<Vec<u8>> = [
        "REL00000.DAT",
        "REL00001.DAT",
        "REL00003.DAT",
        "REL00006.DAT",
    ]
    .iter()
    .map(|f| std::fs::read(format!("{base}{f}")).unwrap())
    .collect();
    let rels: Vec<(u16, &[u8], bool)> = vec![
        (0, &r[0], true),
        (1, &r[1], false),
        (3, &r[2], false),
        (6, &r[3], false),
    ];
    for ctx in [vec![(0u32, 241269u32)], vec![(0u32, 10u32)]] {
        match check_city_list(&lid, &rels, &ctx) {
            Err(e) => println!("ctx {ctx:?}: GATE {e:?}"),
            Ok(cl) => println!(
                "ctx {ctx:?}: ne={} keys={} cand={} list={} dropped={}",
                cl.ne,
                cl.keys,
                cl.candidates,
                cl.list.len(),
                cl.dropped.len()
            ),
        }
    }
    println!("--- street list (our LID20006 + our REL00001, city ctx [0..10)) ---");
    let lid6 = std::fs::read(format!("{base}LID20006.DAT")).unwrap();
    match check_street_list(&lid6, &r[1], &[(0, 10)]) {
        Err(e) => println!("GATE {e:?}"),
        Ok(v) => println!("streets={} first {:?}", v.len(), &v[..v.len().min(8)]),
    }
}
