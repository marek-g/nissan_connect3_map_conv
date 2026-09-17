// Derived tables: address-list name splitting (`addr`), province list (`region`) and
// 'PROVINCE-ADJECTIVE, CITY' city entries (`city_prefixed`) — the sources the car actually
// browses when it lists streets of a city and asks "which province?".

use lid2dump::sqlite_export::{ExportElement, ExportFile};
use rusqlite::Connection;

fn elem(elem: u32, name: &str, sort_name: &str) -> ExportElement {
    ExportElement {
        elem,
        block: 0,
        name: name.into(),
        sort_name: sort_name.into(),
        category: 0,
        has_pos: false,
        x_pau: 0,
        y_pau: 0,
        belonging: 0xffff_ffff,
    }
}

fn list(name: &str, list_id: u16, elements: Vec<ExportElement>) -> ExportFile {
    ExportFile {
        path: format!("/tmp/{name}.DAT"),
        name: name.into(),
        kind: "namelist".into(),
        list_id: Some(list_id),
        country: Some("POL".into()),
        block_count: Some(1),
        origin: None,
        coords_valid_override: None,
        elements,
        rel: None,
        hnr: Vec::new(),
        block_cells: Vec::new(),
        cellmap: Vec::new(),
        mirrors: Vec::new(),
        crossings: Vec::new(),
    }
}

#[test]
fn car_view_city_streets_via_addr_regions() {
    // city list: plain, ASCII/diacritics pair, province-prefixed, and its bare twin
    let city = list(
        "LID20001",
        2,
        vec![
            elem(0, "KRZESZOWICE", "08 500 KRZESZOWICE"),
            elem(3, "KRAKOW", "43 198 KRAKOW\t43 198 KRAKOW\tKRAKÓW"),
            elem(7, "MAZOWIECKI, GRODZISK", "08 505 GRODZISK"),
            elem(8, "GRODZISK", "08 506 GRODZISK"),
        ],
    );
    let region = list(
        "LID20005",
        9,
        vec![
            elem(26, "WOJ. MAZOWIECKIE", "WOJ. MAZOWIECKIE"),
            elem(32, "WOJ. ŚLĄSKIE", "WOJ. SLASKIE\tWOJ. ŚLĄSKIE"),
            elem(3, "WOIWODSCHAFT MASOWIEN", "WOIWODSCHAFT MASOWIEN"),
        ],
    );
    let addr = list(
        "LID20000",
        129,
        vec![
            elem(
                1,
                "KRZESZOWICE, ULICA ZBICKA 3",
                "KRZESZOWICE, ULICA ZBICKA 3",
            ),
            elem(
                2,
                "KRAKOW, ULICA GROMADY GRUDZIAZ 21",
                "KRAKOW, ULICA GROMADY GRUDZIAZ 21",
            ),
            elem(
                3,
                "MAZOWIECKI, GRODZISK, GLUROWNA 5",
                "MAZOWIECKI, GRODZISK, GLUROWNA 5",
            ),
            elem(4, "GRODZISK, SPACERNA", "GRODZISK, SPACERNA"),
        ],
    );
    // the empty LID40002 also claims list_id 2 — the views must tolerate it
    let empty = list("LID40002", 2, Vec::new());

    let mut conn = Connection::open_in_memory().unwrap();
    lid2dump::sqlite_export::write_to(&mut conn, &[city, region, addr, empty]).unwrap();

    let addr_rows: Vec<(String, String, Option<String>)> = conn
        .prepare("SELECT city, street, house_number FROM addr ORDER BY elem")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        addr_rows,
        vec![
            (
                "KRZESZOWICE".into(),
                "ULICA ZBICKA".into(),
                Some("3".into())
            ),
            (
                "KRAKOW".into(),
                "ULICA GROMADY GRUDZIAZ".into(),
                Some("21".into())
            ),
            (
                "MAZOWIECKI, GRODZISK".into(),
                "GLUROWNA".into(),
                Some("5".into())
            ),
            ("GRODZISK".into(), "SPACERNA".into(), None),
        ],
        "got {addr_rows:?}"
    );

    let prefixed: Vec<(String, String, String, String)> = conn
        .prepare(
            "SELECT p.base_name, p.adjective, r.name, p.base_ascii
               FROM city_prefixed p JOIN region r ON r.file_id = p.region_file_id AND r.elem = p.region_elem",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        prefixed,
        vec![(
            "GRODZISK".into(),
            "MAZOWIECKI".into(),
            "WOJ. MAZOWIECKIE".into(),
            "GRODZISK".into()
        )]
    );

    let car_list: Vec<(String, String)> = conn
        .prepare(
            "SELECT DISTINCT source, name FROM v_city_street
              WHERE city_name IN ('KRZESZOWICE') ORDER BY source, name",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(
        car_list,
        vec![("addr".into(), "ULICA ZBICKA".into())],
        "got {car_list:?}"
    );

    let krakow: i64 = conn
        .prepare(
            "SELECT COUNT(*) FROM v_city_street WHERE city_name = 'KRAKÓW' AND name = 'ULICA GROMADY GRUDZIAZ'",
        )
        .unwrap()
        .query_row([], |r| r.get(0))
        .unwrap();
    assert_eq!(
        krakow, 0,
        "KRAKOW addr entry must NOT alias KRAKÓW diacritic name"
    );
    let krakow_ascii: i64 = conn
        .prepare(
            "SELECT COUNT(*) FROM v_city_street WHERE city_name = 'KRAKOW' AND name = 'ULICA GROMADY GRUDZIAZ'",
        )
        .unwrap()
        .query_row([], |r| r.get(0))
        .unwrap();
    assert_eq!(
        krakow_ascii, 1,
        "ASCII city name must carry the address-derived street"
    );
}
