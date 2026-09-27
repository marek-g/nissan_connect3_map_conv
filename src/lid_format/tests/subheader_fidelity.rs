//! Device-fidelity tests for `NLNameList::LoadHeader` (00e0e63c) replica.
//! The device cross-checks EVERY subheader field against the streams: each of the 7
//! streams must decode to EXACTLY the declared count and consume its window to the
//! last byte (sec6 ends at the subheader end). elem_count is a u32 (stock LID20001
//! stores 241269 -> a u16 read truncates to 44661 and `elem-1` bounds go wrong).
use lid_format::city::{build_city_file, CityEntry};
use lid_format::device_sim::check_name_list_header;

const POL: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                   CRYPTNAV/DATA/DATA/LID/CCP/POL/";

fn stock(name: &str) -> Vec<u8> {
    std::fs::read(format!("{POL}{name}")).expect("read stock")
}

#[test]
fn stock_name_list_files_pass_the_device_load_header() {
    for name in [
        "LID20000.DAT",
        "LID20001.DAT",
        "LID20002.DAT",
        "LID20003.DAT",
        "LID20004.DAT",
        "LID20005.DAT",
        "LID20006.DAT",
    ] {
        let b = stock(name);
        let h = check_name_list_header(name, &b)
            .unwrap_or_else(|g| panic!("{name} must load like the device: {}", g.0));
        assert!(h.nb > 0, "{name}: no blocks");
        assert_eq!(h.shift, 8, "{name}: position shift");
    }
}

#[test]
fn elem_count_is_a_u32_not_a_truncated_u16() {
    let h = check_name_list_header("LID20001.DAT", &stock("LID20001.DAT")).unwrap();
    assert_eq!(
        h.elem, 241_269,
        "stock city elem must not truncate to 44661"
    );
    let h = check_name_list_header("LID20006.DAT", &stock("LID20006.DAT")).unwrap();
    assert_eq!(h.elem, 974_871, "stock street elem");
}

#[test]
fn our_generated_city_file_passes_the_device_load_header() {
    let cities: Vec<CityEntry> = [("BYDGOSZCZ", 18.0084f64, 53.1235f64)]
        .iter()
        .map(|(n, lo, la)| CityEntry::from_deg(n, *lo, *la))
        .collect();
    let bytes = build_city_file(&cities, &stock("LID20001.DAT"));
    let h = check_name_list_header("ours.lid", &bytes).expect("our file must load too");
    assert_eq!(h.elem, 1, "elem = our element count, not the stock one");
    assert_eq!(h.nb, 1);
}

fn synthetic_name_list(elem: u32, nb: u16) -> Vec<u8> {
    // minimal valid container: header 0x26 + subheader [u32 elem][u16 nb,nrec,nrel,
    // cnt5,cnt6,shift][origin x2][7x{u8,u32}] with all streams empty at blob_end.
    let sub_sz = 0x18 + 7 * 5;
    let mut b = vec![0u8; 0x26 + sub_sz];
    let sub = 0x26u32;
    b[0x10..0x14].copy_from_slice(&sub.to_le_bytes());
    b[0x14..0x18].copy_from_slice(&(sub_sz as u32).to_le_bytes());
    b[0x26..0x2a].copy_from_slice(&elem.to_le_bytes());
    let w16 =
        |b: &mut Vec<u8>, off: usize, v: u16| b[off..off + 2].copy_from_slice(&v.to_le_bytes());
    w16(&mut b, 0x26 + 4, nb); // nb
    w16(&mut b, 0x26 + 8, 0); // nrel
    w16(&mut b, 0x26 + 0x0e, 8); // shift
    let tbl = 0x26 + 0x18;
    let end = (0x26 + sub_sz) as u32;
    for i in 0..7 {
        b[tbl + i * 5] = 0x11;
        b[tbl + i * 5 + 1..tbl + i * 5 + 5].copy_from_slice(&end.to_le_bytes());
    }
    b
}

#[test]
fn synthetic_large_elem_parses_and_tampering_is_rejected() {
    let b = synthetic_name_list(65_541, 0);
    let h = check_name_list_header("synth", &b).expect("valid synthetic file");
    assert_eq!(h.elem, 65_541, "u32 elem past the u16 range");

    // declare one block but give stream0/sec1 an empty window: the device's exact-count
    // rule must reject it (this is the class of bug a lenient parser would swallow).
    let mut bad = b.clone();
    bad[0x2a..0x2c].copy_from_slice(&1u16.to_le_bytes());
    assert!(check_name_list_header("synth-bad", &bad).is_err());
}
