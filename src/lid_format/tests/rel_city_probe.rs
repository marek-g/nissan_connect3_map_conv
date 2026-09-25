//! Probe: what do the stock city-side REL matrices return for the stock element ids of the
//! 10 test cities? Answers whether a from-scratch city file needs its own REL00000/1/3/6.

use lid_format::rel::{get_relations, RelIndex};

const POL: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                   CRYPTNAV/DATA/DATA/LID/CCP/POL";

fn rel(name: &str) -> (Vec<u8>, RelIndex) {
    let b = std::fs::read(format!("{POL}/{name}")).unwrap();
    let idx = RelIndex::parse(&b).unwrap();
    (b, idx)
}

#[test]
#[ignore]
fn city_relation_domains() {
    // stock ids of the 10 cities incl. ascii/diacritic variants (lid2dump c10_stock.json)
    let stock_ids: Vec<u32> = vec![
        194288, 199864, 199865, 208837, 208838, 211986, 211987, 212340, 222141, 222142, 222143,
        222146, 222147, 222148, 223516, 223517, 231069, 231070, 233759, 236833, 236839, 240888,
        240889,
    ];
    // REL00000 (2<->2): what does a stock city relate to inside its own list?
    let (b0, i0) = rel("REL00000.DAT");
    for id in [199864u32, 199865, 222141, 222146, 233759] {
        let bys = get_relations(&b0, &i0, true, id, id + 1).unwrap();
        let byt = get_relations(&b0, &i0, false, id, id + 1).unwrap();
        println!("REL00000 {id}: col->tgt {bys:?} | row: city<-? src {byt:?}");
    }
    // REL00002 (2->12): POI rows for a city
    let (b2, i2) = rel("REL00002.DAT");
    for id in [233759u32] {
        let r = get_relations(&b2, &i2, true, id, id + 1).unwrap();
        println!(
            "REL00002 city {id}: {} POIs, first {:?}",
            r.len(),
            &r[..r.len().min(5)]
        );
    }
    // REL00003 (10<->2) and REL00006 (9<->2): district / region of our cities
    for file in ["REL00003.DAT", "REL00006.DAT"] {
        let (b, i) = rel(file);
        for id in &stock_ids {
            let r = get_relations(&b, &i, false, *id, id + 1).unwrap();
            let v: Vec<u32> = r.iter().map(|p| p.1).collect();
            println!("{file} city {id}: districts/regions {v:?}");
        }
    }
    // REL00001 (3->2): street count for one stock city
    let (b1, i1) = rel("REL00001.DAT");
    for id in [233759u32] {
        let r = get_relations(&b1, &i1, false, id, id + 1).unwrap();
        println!(
            "REL00001 city {id}: {} streets, first {:?}",
            r.len(),
            &r[..r.len().min(6)]
        );
    }
    // sanity: stock ids all exist within REL00003 target domain (714 src x 241269 tgt)
    let _ = stock_ids;
}
