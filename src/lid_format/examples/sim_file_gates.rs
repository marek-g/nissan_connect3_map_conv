//! File-admission gate runner: replays `NLNameList::LoadHeader` (00e0e63c) and the REL
//! gates (`NLRelMatrixFile::DecodeSubHeader` + `bCheckAndCalculate`) over stock card
//! files and our generated files, printing the first failing device gate per file.

use lid_format::device_sim::{check_name_list_header, RelSim};

const POL: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";

fn main() {
    let mut fails = 0usize;
    println!("== name-list files (NLNameList::LoadHeader gate) ==");
    let nl_files: Vec<(String, String)> = vec![
        (format!("{POL}LID20001.DAT"), "STOCK city".into()),
        (format!("{POL}LID20002.DAT"), "STOCK district".into()),
        (format!("{POL}LID20006.DAT"), "STOCK street".into()),
        (format!("{POL}LID40006.DAT"), "STOCK street-cross".into()),
        ("/tmp/opencode/zzmin1.DAT".into(), "OUR TEST file".into()),
        (
            "/tmp/opencode/old_city.DAT".into(),
            "OUR 10-city (records build)".into(),
        ),
        (
            "/tmp/opencode/zzcity10.DAT".into(),
            "OUR 10-city (quantized build)".into(),
        ),
    ];
    for (path, tag) in nl_files {
        let Ok(b) = std::fs::read(&path) else {
            println!("  {tag:<34} MISSING {path}");
            continue;
        };
        let nm = path.rsplit('/').next().unwrap().to_string();
        match check_name_list_header(&nm, &b) {
            Ok(h) => println!(
                "  {tag:<34} PASS  elem={} nb={} nrec={} nrel={} shift={} origin=({},{}) rel={:?} cats={:?}",
                h.elem, h.nb, h.nrec, h.nrel, h.shift, h.origin.0, h.origin.1, h.rel_files, h.cats
            ),
            Err(g) => {
                println!("  {tag:<34} REJECT {g:?}");
                fails += 1;
            }
        }
    }
    println!("== REL files (DecodeSubHeader + bCheckAndCalculate gates) ==");
    for i in [0u32, 1, 2, 3, 6] {
        let p = format!("{POL}REL{i:05}.DAT");
        let tag = format!("STOCK REL{i:05}");
        let Ok(b) = std::fs::read(&p) else {
            println!("  {tag:<34} MISSING");
            continue;
        };
        match RelSim::open(&tag, &b) {
            Ok(r) => println!("  {tag:<34} PASS  d={:?} geo={:?}", r.d, r.geo),
            Err(g) => {
                println!("  {tag:<34} REJECT {g:?}");
                fails += 1;
            }
        }
    }
    for i in [0u32, 1, 3, 6] {
        let p = format!("/tmp/opencode/REL{i:05}.DAT");
        let tag = format!("OUR REL{i:05}");
        let Ok(b) = std::fs::read(&p) else {
            println!("  {tag:<34} MISSING {p}");
            continue;
        };
        match RelSim::open(&tag, &b) {
            Ok(r) => println!("  {tag:<34} PASS  d={:?} geo={:?}", r.d, r.geo),
            Err(g) => {
                println!("  {tag:<34} REJECT {g:?}");
                fails += 1;
            }
        }
    }
    println!("failing gates: {fails}");
    std::process::exit(if fails == 0 { 0 } else { 1 });
}
