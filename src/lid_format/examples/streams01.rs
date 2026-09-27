use lid_format::device_sim::*;
fn main() {
    for (nm, path) in [("STOCK-1","/tmp/opencode/stock1r.DAT"),("OURS-1","trials/28 - LID/raw/LID20001.DAT"),("STOCK-6","/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20006.DAT"),("OURS-6","trials/28 - LID/raw/LID20006.DAT")] {
        let b = std::fs::read(path).unwrap();
        let h = check_name_list_header(nm, &b).unwrap();
        println!("{nm}: nb={} blob_end={} file={}", h.nb, h.blob_end, b.len());
        println!("  s0(starts/ends?): {:?}", &h.streams[0][..h.streams[0].len().min(6)]);
        println!("  s1             : {:?}", &h.streams[1][..h.streams[1].len().min(6)]);
        let n = h.nb as usize;
        if n>=3 { println!("  s0 tail: {:?}  s1 tail: {:?}", &h.streams[0][n-3..], &h.streams[1][n-3..]); }
    }
}
