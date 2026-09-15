use lid_format::{encode, read, NameEntry};

fn roundtrip(names: &[&str]) {
    // Two cities exercise the city-grouped blocks (positions stored position-city, §12.5).
    let entries: Vec<NameEntry> = names
        .iter()
        .enumerate()
        .map(|(i, n)| NameEntry {
            label: n.to_string(),
            x_pau: (i as i32) * 100,
            y_pau: (i as i32) * -7,
            city: if i % 2 == 0 {
                Some((9_000_000, 8_000_000))
            } else {
                Some((300_000_000, 250_000_000))
            },
        })
        .collect();
    let bytes = encode(&entries);
    assert!(
        lid_format::is_name_list(&bytes),
        "encoder output not recognized as a name-list"
    );
    let nl = read(&bytes).expect("read failed");

    let mut want: Vec<(String, i32, i32)> = entries
        .iter()
        .map(|e| {
            let a = e.city.unwrap();
            (e.label.clone(), e.x_pau - a.0, e.y_pau - a.1) // stored relative to city (device model)
        })
        .collect();
    want.sort();
    let mut got: Vec<(String, i32, i32)> = nl
        .elements
        .iter()
        .map(|e| (e.name.clone(), e.x_pau, e.y_pau))
        .collect();
    got.sort();
    assert_eq!(want, got, "round-trip mismatch");
    assert!(
        nl.origin.is_some(),
        "city flavor must advertise the file-level anchor (flags bit 0x80000)"
    );
    assert_eq!(
        nl.block_count >= 2,
        names.len() >= 2,
        "positions must group per city into separate blocks"
    );
    assert!(nl.elements.iter().all(|e| e.has_pos));
}

#[test]
fn basic() {
    roundtrip(&["MARIA", "MARIA KONOPNICKA", "PIOTRA SKARGI", "3 MAJA", "A"]);
}

#[test]
fn prefixes() {
    // "B" is a strict prefix of "BAR"; both must survive as elements (terminator makes each a leaf).
    roundtrip(&["B", "BAR", "BARKA", "BA"]);
}

#[test]
fn unicode() {
    roundtrip(&["ŻWIŘINA Ω", "ŁÓDŹ", "ULICA ŚW. JANA"]);
}

#[test]
fn gazetteer_has_no_positions() {
    // The stock settlement gazetteer (LID20000) carries no coordinates: city=None must write the empty
    // 0x407 flavor and a -1/-1 origin, which `read` reports as has_pos=false / origin=None.
    let entries: Vec<NameEntry> = ["KRAKOW", "WARSZAWA", "KRAKUSY"]
        .iter()
        .enumerate()
        .map(|(i, n)| NameEntry {
            label: n.to_string(),
            x_pau: 100 * i as i32,
            y_pau: -7 * i as i32,
            city: None,
        })
        .collect();
    let bytes = encode(&entries);
    let nl = read(&bytes).expect("read failed");
    assert_eq!(nl.element_count as usize, 3);
    assert!(
        nl.origin.is_none(),
        "gazetteer origin must read back as None (stock -1/-1)"
    );
    assert!(nl.elements.iter().all(|e| !e.has_pos));
}

#[test]
fn many_blocks() {
    // High-entropy labels (little prefix sharing) so the trie splits across several blocks.
    let entries: Vec<NameEntry> = (0..30_000)
        .map(|i| {
            let mut h = (i as u32).wrapping_mul(2654435761);
            let label = format!("UL-{:09}", h % 1_000_000_000);
            NameEntry {
                label,
                x_pau: i as i32 * 13,
                y_pau: -(i as i32) * 11,
                city: Some((0, 0)),
            }
        })
        .collect();
    let bytes = encode(&entries);
    let nl = read(&bytes).unwrap();
    assert!(
        nl.block_count > 1,
        "expected multi-block output, got {}",
        nl.block_count
    );
    let mut want: Vec<(String, i32, i32)> = entries
        .iter()
        .map(|e| (e.label.clone(), e.x_pau, e.y_pau))
        .collect();
    want.sort();
    let mut got: Vec<(String, i32, i32)> = nl
        .elements
        .iter()
        .map(|e| (e.name.clone(), e.x_pau, e.y_pau))
        .collect();
    got.sort();
    assert_eq!(
        want.len(),
        got.len(),
        "element count mismatch (blocks dropped elements?)"
    );
    assert_eq!(want, got);
}
