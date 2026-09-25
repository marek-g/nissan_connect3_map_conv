fn main() {
    let b = std::fs::read("/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/REL00003.DAT").unwrap();
    let idx = lid_format::rel::RelIndex::parse(&b).unwrap();
    let v = lid_format::rel::get_relations(&b, &idx, true, 457, 458).unwrap();
    eprintln!("{} pairs", v.len());
}
