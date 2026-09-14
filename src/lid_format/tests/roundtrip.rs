use lid_format::{encode, read, NameEntry};

fn roundtrip(names: &[&str]) {
    let entries: Vec<NameEntry> = names
        .iter()
        .enumerate()
        .map(|(i, n)| NameEntry { label: n.to_string(), x_pau: (i as i32) * 100, y_pau: (i as i32) * -7 })
        .collect();
    let bytes = encode(&entries);
    assert!(lid_format::is_name_list(&bytes), "encoder output not recognized as a name-list");
    let nl = read(&bytes).expect("read failed");

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
    assert_eq!(want, got, "round-trip mismatch");
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
fn many_blocks() {
    // High-entropy labels (little prefix sharing) so the trie splits across several blocks.
    let entries: Vec<NameEntry> = (0..30_000)
        .map(|i| {
            let mut h = (i as u32).wrapping_mul(2654435761);
            let label = format!("UL-{:09}", h % 1_000_000_000);
            NameEntry { label, x_pau: i as i32 * 13, y_pau: -(i as i32) * 11 }
        })
        .collect();
    let bytes = encode(&entries);
    let nl = read(&bytes).unwrap();
    assert!(nl.block_count > 1, "expected multi-block output, got {}", nl.block_count);
    let mut want: Vec<(String, i32, i32)> = entries.iter().map(|e| (e.label.clone(), e.x_pau, e.y_pau)).collect();
    want.sort();
    let mut got: Vec<(String, i32, i32)> = nl.elements.iter().map(|e| (e.name.clone(), e.x_pau, e.y_pau)).collect();
    got.sort();
    assert_eq!(want.len(), got.len(), "element count mismatch (blocks dropped elements?)");
    assert_eq!(want, got);
}
