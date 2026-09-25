use lid_format::{pos_shift, read};

fn main() {}

#[test]
#[ignore]
fn find_szczecin() {
    let path = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20001.DAT";
    let b = std::fs::read(path).unwrap();
    let nl = read(&b).unwrap();
    let (ox, oy) = nl.origin.unwrap();
    let sh = pos_shift(&b) as u32;
    let deg = 2f64.powi(31) / 180.0;
    for (i, e) in nl.elements.iter().enumerate() {
        let lon = (ox as i64 + ((e.x_pau as i64) << sh)) as f64 / deg;
        let lat = (oy as i64 + ((e.y_pau as i64) << sh)) as f64 / deg;
        if (13.9..15.3).contains(&lon) && (53.0..53.8).contains(&lat) {
            println!("id {i:>7} {lon:.4}E {lat:.4}N {:?}", e.name);
        }
    }
}
