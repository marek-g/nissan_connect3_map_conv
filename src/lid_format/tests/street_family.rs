//! Device-sim regression for the from-scratch STREET stage: the generated LID20006 shape, its
//! REL00001 owner matrix and the HNR GenAttr file must pass the replica of the device read path
//! (LoadHeader + SetDataBlock + REL collection + enGetHnrIndices/enGetHnr) exactly and completely.
use lid_format::city::CityEntry;
use lid_format::device_sim::{
    check_block_load, check_hnr, check_name_list_header, check_street_list,
};
use lid_format::street::{build_hnr_file, build_street_file, build_street_relations, HnrRange};

const POL: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                   CRYPTNAV/DATA/DATA/LID/CCP/POL/";

fn stock(name: &str) -> Vec<u8> {
    std::fs::read(format!("{POL}{name}")).expect("read stock")
}

fn streets6() -> (Vec<CityEntry>, Vec<u32>, Vec<HnrRange>) {
    let specs: [(&str, u32, f64, f64); 6] = [
        ("EGZAMINACYJNA", 4, 16.9252, 52.4064),
        ("PRZYKŁADOWA", 2, 19.9450, 50.0647),
        ("SIESTA", 7, 21.0118, 52.2298),
        ("TESTOWA", 7, 21.0218, 52.2398),
        ("WIDMOWA", 1, 18.6464, 54.3520),
        ("ZABAWNA", 9, 19.4560, 51.7592),
    ];
    let entries = specs
        .iter()
        .map(|(n, _, lo, la)| CityEntry::from_deg(n, *lo, *la))
        .collect::<Vec<_>>();
    let city_of = specs.iter().map(|&(_, c, _, _)| c).collect();
    // ids = TEIC order = byte order of the names above (already sorted, ids 0..5)
    let ranges = vec![
        HnrRange {
            street_elem: 0,
            odd: (1, 15),
            even: (2, 16),
        },
        HnrRange {
            street_elem: 1,
            odd: (1, 11),
            even: (0, 0),
        },
        HnrRange {
            street_elem: 2,
            odd: (1, 5),
            even: (2, 6),
        },
        HnrRange {
            street_elem: 3,
            odd: (1, 9),
            even: (0, 0),
        },
        HnrRange {
            street_elem: 4,
            odd: (0, 0),
            even: (2, 8),
        },
        HnrRange {
            street_elem: 5,
            odd: (3, 9),
            even: (0, 0),
        },
    ];
    (entries, city_of, ranges)
}

#[test]
fn street_file_passes_all_device_gates() {
    let (entries, city_of, ranges) = streets6();
    let (data, _origin) = build_street_file(&entries, &stock("LID20006.DAT"));
    let h = check_name_list_header("zzstreet6.DAT", &data).expect("LoadHeader");
    assert_eq!((h.elem as usize, h.nb), (6, 1));
    assert_eq!(h.cats, vec![3]);
    let bl = check_block_load("zzstreet6.DAT", &data, &h, 0).expect("SetDataBlock");
    assert!(bl.danger.is_empty(), "overruns: {:?}", bl.danger);
    assert_eq!(bl.valid_dest_popcount, 6, "validDestination all-set");

    // REL00001 owner fetch per city, like the street browser does.
    let rel1 = build_street_relations(&city_of, 10).expect("rel1");
    let by_city = |c: u32| check_street_list(&data, &rel1, &[(c, c + 1)]).unwrap();
    assert_eq!(by_city(7), vec![2, 3]); // SIESTA, TESTOWA
    assert_eq!(by_city(2), vec![1]); //  PRZYKŁADOWA
    assert_eq!(by_city(1), vec![4]); //  WIDMOWA
    assert_eq!(by_city(4), vec![0]); //  EGZAMINACYJNA
    assert_eq!(by_city(9), vec![5]); //  ZABAWNA
    assert_eq!(by_city(0), Vec::<u32>::new()); // BYDGOSZCZ: none

    // HNR device replay.
    let hnr = build_hnr_file(6, &ranges, &stock("LID40006.DAT")).expect("hnr");
    let recs = check_hnr(&hnr).expect("device HNR replay");
    let want: Vec<u32> = (1..=15).step_by(2).chain((2..=16).step_by(2)).collect();
    let got: Vec<u32> = recs
        .iter()
        .filter(|r| r.street == 0)
        .map(|r| r.number)
        .collect();
    assert_eq!(got.len(), 16);
    assert!(got.iter().all(|n| want.contains(n)));
    for r in &recs {
        let rng = ranges[r.street as usize];
        assert!(
            (rng.odd.1 > 0 && r.number >= rng.odd.0 && r.number <= rng.odd.1)
                || (rng.even.1 > 0 && r.number >= rng.even.0 && r.number <= rng.even.1),
            "number {} outside street {} ranges",
            r.number,
            r.street
        );
        if rng.odd.1 == 0 {
            assert!(r.even);
        }
    }
    let _ = entries;
}

#[test]
fn stock_hnr_file_replays() {
    // model validation: the device read path over the real stock GenAttr file must decode
    // every record (2.35M) with a consistent owner walk.
    let recs = check_hnr(&stock("LID40006.DAT")).expect("stock HNR replay");
    assert!(recs.len() > 2_000_000, "stock record count {}", recs.len());
}
