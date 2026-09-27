use lid_format::device_sim::*;
fn run(nm: &str, lid: &[u8], rels: &[(u16, &[u8], bool)]) {
    let ctx = [(0u32, 241269u32)];
    match check_city_list(lid, rels, &ctx) {
        Ok(cl) => println!(
            "{nm}: ne={} keys={} candidates={} list={} dropped={} danger={}",
            cl.ne,
            cl.keys,
            cl.candidates,
            cl.list.len(),
            cl.dropped.len(),
            cl.danger.len()
        ),
        Err(e) => println!("{nm}: GATE {e:?}"),
    }
}
fn main() {
    let fw = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";
    let ours = std::fs::read("trials/28 - LID/raw/LID20001.DAT").unwrap();
    let stock = std::fs::read("/tmp/opencode/stock1r.DAT").unwrap();
    let s0 = std::fs::read(format!("{fw}REL00000.DAT")).unwrap();
    let s1 = std::fs::read(format!("{fw}REL00001.DAT")).unwrap();
    let s3 = std::fs::read(format!("{fw}REL00003.DAT")).unwrap();
    let s6 = std::fs::read(format!("{fw}REL00006.DAT")).unwrap();
    let stockrels: Vec<(u16, &[u8], bool)> = vec![
        (0, &s0, true),
        (1, &s1, false),
        (3, &s3, false),
        (6, &s6, false),
    ];
    let o = "trials/28 - LID/raw";
    let o0 = std::fs::read(format!("{o}/REL00000.DAT")).unwrap();
    let o1 = std::fs::read(format!("{o}/REL00001.DAT")).unwrap();
    let o3 = std::fs::read(format!("{o}/REL00003.DAT")).unwrap();
    let o6 = std::fs::read(format!("{o}/REL00006.DAT")).unwrap();
    let ourrels: Vec<(u16, &[u8], bool)> = vec![
        (0, &o0, true),
        (1, &o1, false),
        (3, &o3, false),
        (6, &o6, false),
    ];
    // CONTROL: stock LID + stock REL (what the card does today on stock restore):
    run("stockLID+stockREL", &stock, &stockrels);
    // V1 exactly as flashed (our LID20001 over stock set):
    run("V1:  ourLID +stockREL", &ours, &stockrels);
    // 2b planned next (our LID + our REL):
    run("2b:  ourLID +  ourREL", &ours, &ourrels);
}
