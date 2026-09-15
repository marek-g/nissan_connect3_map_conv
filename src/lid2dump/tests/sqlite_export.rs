// Round-trip test of the relational SQLite export against hand-built model data.

use lid2dump::sqlite_export::{
    export, Cell, ExportElement, ExportFile, HnrRow, MirrorTable, RelOut,
};

fn elem(
    elem: u32,
    block: usize,
    name: &str,
    sort_name: &str,
    pos: Option<(i32, i32)>,
) -> ExportElement {
    ExportElement {
        elem,
        block,
        name: name.into(),
        sort_name: sort_name.into(),
        category: 0,
        has_pos: pos.is_some(),
        x_pau: pos.map_or(0, |p| p.0),
        y_pau: pos.map_or(0, |p| p.1),
        belonging: 0xffff_ffff,
    }
}

fn export_file(name: &str, kind: &str, list_id: Option<u16>) -> ExportFile {
    ExportFile {
        path: format!("/tmp/{name}.DAT"),
        name: name.into(),
        kind: kind.into(),
        list_id,
        country: Some("POL".into()),
        block_count: Some(1),
        origin: Some((268_000_000, 600_000_000)),
        coords_valid_override: None,
        elements: Vec::new(),
        rel: None,
        hnr: Vec::new(),
        mirrors: Vec::new(),
    }
}

#[test]
fn export_views_and_joins() {
    let mut city = export_file("LID20001", "namelist", Some(2));
    city.elements = vec![
        elem(
            0,
            0,
            "KRZESZOWICE",
            "08 500 KRZESZOWICE",
            Some((12_000, 30_000)),
        ),
        elem(
            1,
            0,
            "TENCZYN",
            "08 501 TENCZYN\tTĘCZYN",
            Some((1_000, 2_000)),
        ),
    ];
    let mut street = export_file("LID20006", "namelist", Some(3));
    street.elements = vec![
        elem(
            0,
            0,
            "TENCZYŃSKA, ULICA",
            "TENCZYNSKA, ULICA",
            Some((-4_000, 900)),
        ),
        elem(1, 0, "SIENKIEWICZA, ULICA", "SIENKIEWICZA, ULICA", None),
    ];
    let mut rel = export_file("REL00001", "rel", Some(29));
    rel.origin = None;
    rel.block_count = None;
    rel.rel = Some(RelOut {
        d0: 3,
        d1: 2,
        pairs: vec![(0, 0), (1, 0)], // both streets belong to city elem 0
    });
    let mut hnr = export_file("LID40006", "gen_attr", Some(40006));
    hnr.origin = None;
    hnr.block_count = None;
    hnr.hnr = vec![
        HnrRow {
            elem: 0,
            addr_to_street: Some(0),
            house_number: Some(7),
        },
        HnrRow {
            elem: 1,
            addr_to_street: Some(1),
            house_number: Some(3),
        },
    ];
    let mut poi = export_file("GLOB_POI", "sqlite", None);
    poi.origin = None;
    poi.block_count = None;
    poi.mirrors = vec![MirrorTable {
        name: "GLOB_POI__POI".into(),
        columns: vec!["ID".into(), "NAME".into(), "NOTE".into()],
        rows: vec![
            vec![Cell::Int(1), Cell::Text("Kapliczka".into()), Cell::Null],
            vec![
                Cell::Int(2),
                Cell::Text("Młyn".into()),
                Cell::Blob(vec![1, 2, 3]),
            ],
        ],
    }];

    let tmp = std::env::temp_dir().join("lid2dump_export_test.db");
    let _ = std::fs::remove_file(&tmp);
    export(&tmp, &[city, street, rel, hnr, poi]).expect("export");
    let db = rusqlite::Connection::open(&tmp).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    let n_fk = {
        let mut s = db.prepare("PRAGMA foreign_key_check").unwrap();
        s.query_map([], |_| Ok(()))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .len()
    };
    assert_eq!(n_fk, 0, "no FK violations");

    // element count + raw fidelity (TAB multi-line kept raw, NULL category/coords)
    let n: i64 = db
        .query_row("SELECT COUNT(*) FROM element", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 4);
    let (name, sort): (String, String) = db
        .query_row("SELECT name, sort_name FROM v_city WHERE elem=1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(name, "TENCZYN");
    assert_eq!(sort, "08 501 TENCZYN\tTĘCZYN");
    let lat_missing: Option<f64> = db
        .query_row(
            "SELECT latitude FROM v_element WHERE file='LID20006' AND elem=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(lat_missing.is_none(), "has_pos=0 must have NULL lat");

    // street→city join via rel
    let city_name: String = db
        .query_row("SELECT city_name FROM v_street WHERE elem=0", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(city_name, "KRZESZOWICE");

    // address join chain hnr→street→city + honest coordinates
    let row: (Option<String>, Option<String>, Option<i64>, Option<f64>) = db
        .query_row(
            "SELECT city, street, house_number, latitude FROM v_address WHERE hnr_elem=0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();
    assert_eq!(row.0.as_deref(), Some("KRZESZOWICE"));
    assert_eq!(row.1.as_deref(), Some("TENCZYŃSKA, ULICA"));
    assert_eq!(row.2, Some(7));
    let expect = (600_000_000i64 + 900) as f64 * 180.0 / 2147483648.0;
    assert!((row.3.unwrap() - expect).abs() < 1e-12);

    // multi-block file → coords NULL (honesty rule via coords_valid)
    // bundled query pack
    let q: i64 = db
        .query_row("SELECT COUNT(*) FROM queries", [], |r| r.get(0))
        .unwrap();
    assert!(q >= 7);
    let gaz = db
        .query_row("SELECT sql FROM queries WHERE name='gazetteer'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    assert!(gaz.contains("zip_code"));

    // mirrored embedded table stays raw (incl. NULL + BLOB)
    let blob: Vec<u8> = db
        .query_row("SELECT NOTE FROM GLOB_POI__POI WHERE ID=2", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(blob, vec![1, 2, 3]);

    // completeness diagnostics see the gaps
    let miss: Vec<(String, i64)> = {
        let mut s = db
            .prepare("SELECT file, without_pos FROM v_completeness WHERE without_pos > 0")
            .unwrap();
        s.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(miss, vec![("LID20006".to_string(), 1)]);

    std::fs::remove_file(&tmp).ok();
}

#[test]
fn multi_block_file_has_no_abs_coords() {
    let mut city = export_file("LID20001", "namelist", Some(2));
    city.block_count = Some(3); // stock-like multi-block file
    city.elements = vec![elem(0, 2, "X", "X", Some((10, 20)))];
    let tmp = std::env::temp_dir().join("lid2dump_export_test2.db");
    let _ = std::fs::remove_file(&tmp);
    export(&tmp, &[city]).unwrap();
    let db = rusqlite::Connection::open(&tmp).unwrap();
    let lat: Option<f64> = db
        .query_row("SELECT latitude FROM v_city", [], |r| r.get(0))
        .unwrap();
    assert!(lat.is_none());
    let raw: Option<i64> = db
        .query_row("SELECT x_pau FROM v_city", [], |r| r.get(0))
        .unwrap();
    assert_eq!(raw, Some(10)); // raw deltas still ship
    std::fs::remove_file(&tmp).ok();
}
