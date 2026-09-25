// Flash-test generator: from-scratch LID20001 with ONE city (shared lid_format::city writer).
use lid_format::city::{build_city_file, selfcheck, CityEntry};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let stock_path = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                      CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20001.DAT";
    let out_path = args
        .get(1)
        .cloned()
        .unwrap_or("/tmp/opencode/zzmin1.DAT".into());
    let stock = std::fs::read(stock_path).expect("read stock");
    let cities = vec![CityEntry::from_deg("TEST", 21.01, 52.23)];
    let data = build_city_file(&cities, &stock);
    std::fs::write(&out_path, &data).expect("write");
    println!("wrote {} bytes to {}", data.len(), out_path);
    selfcheck(&data, &stock, &cities).expect("selfcheck");
    println!("OK: 1 city verified");
}
