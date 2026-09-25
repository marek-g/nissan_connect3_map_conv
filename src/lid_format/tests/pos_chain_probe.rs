//! Probe: verify the CONFIRMED stock position model `abs = origin + (stored << shift)`
//! (NLPositionAttrVector) against real stock LID20001 cities.
use lid_format::{pos_shift, read};

fn main() {}

#[test]
#[ignore]
fn stock_absolute_positions() {
    let path = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20001.DAT";
    let b = std::fs::read(path).unwrap();
    let nl = read(&b).unwrap();
    let (ox, oy) = nl.origin.unwrap();
    let sh = pos_shift(&b) as u32;
    println!("shift={sh} origin=({ox},{oy})");
    let deg = 2f64.powi(31) / 180.0;
    let want = [
        "WARSZAWA",
        "KRAKÓW",
        "BYDGOSZCZ",
        "ŁÓDŹ",
        "GDAŃSK",
        "RADOM",
        "POZNAŃ",
        "LUBLIN",
        "SZCZECIN",
        "WROCLAW",
    ];
    let mut seen = std::collections::HashSet::new();
    for (i, e) in nl.elements.iter().enumerate() {
        if want.contains(&e.name.as_str()) && seen.insert(e.name.clone()) {
            let ax = ox as i64 + ((e.x_pau as i64) << sh);
            let ay = oy as i64 + ((e.y_pau as i64) << sh);
            println!(
                "id {i:>7} {:<10} abs = {:.4}E {:.4}N",
                e.name,
                ax as f64 / deg,
                ay as f64 / deg
            );
        }
    }
}
