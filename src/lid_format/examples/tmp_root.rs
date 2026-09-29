use lid_format::device_sim::{check_block_load, check_name_list_header};
fn main() {
    let b = std::fs::read("/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20001.DAT").unwrap();
    let h = check_name_list_header("stock", &b).unwrap();
    let bl = check_block_load("stock", &b, &h, 0).unwrap();
    println!("block0: ne={} degs[0]={} domain={}", bl.ne, bl.degs[0], bl.claims[..bl.degs[0] as usize].iter().map(|&x| x as u64).sum::<u64>());
    let mut cur = 0u64;
    for (i, (lab, _)) in bl.root_edges.iter().enumerate() {
        let c = bl.claims[i] as u64;
        println!("edge {i:2} {:>4} claim={c:7} interval=[{cur}, {})", lab, cur + c - 1);
        cur += c;
    }
    // where are our cities? KRAKOW 208838, WARSZAWA 233759, BYDGOSZCZ 194288
    let mut cur = 0u64;
    for id in [194288u64, 208838, 233759] {
        for (i, (lab, _)) in bl.root_edges.iter().enumerate() {
            let c = bl.claims[i] as u64;
            if id >= cur && id < cur + c {
                println!("id {id} -> edge {i} label={lab:?}");
                break;
            }
            cur += c;
        }
    }
}
