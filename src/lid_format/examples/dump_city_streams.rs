use lid_format::device_sim::{block_toc, check_name_list_header};

fn dump(tag: &str, b: &[u8]) {
    let h = check_name_list_header(tag, b).expect("hdr");
    println!("== {tag}: nb={} elems={}", h.nb, h.elem);
    for (t, code, start, count) in block_toc(b, &h, 0) {
        println!("  tag {t:#06x} code {code:#04x} start {start:#x} count {count}");
    }
}

fn main() {
    let p = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20001.DAT";
    dump("STOCK", &std::fs::read(p).unwrap());
    dump(
        "OURS",
        &std::fs::read("trials/28 - LID/raw/LID20001.DAT").unwrap(),
    );
}
