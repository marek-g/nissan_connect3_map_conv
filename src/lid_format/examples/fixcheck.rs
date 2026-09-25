// Verify a grafted city list: read(probe) must equal read(stock) with EXACTLY the graft
// victims removed (names in order), and every surviving element's attributes unchanged.
// Usage: fixcheck <stock.DAT> <probe.DAT> <lost1> <lost2> ...
use lid_format::read;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let stock = read(&std::fs::read(&a[1]).unwrap()).unwrap();
    let probe = read(&std::fs::read(&a[2]).unwrap()).unwrap();
    let lost: Vec<&str> = a[3..].iter().map(|s| s.as_str()).collect();
    // drop exactly one occurrence per lost name in the host block (the LAST one = graft victim,
    // the candidate scan picks from the end). Stock may legitimately hold duplicate spellings.
    let mut drop_ids: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for l in &lost {
        let hit = stock
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.block == 44 && e.name == *l)
            .map(|(i, _)| i)
            .last();
        if let Some(i) = hit {
            drop_ids.insert(i);
        }
    }
    let mut exp: Vec<&lid_format::Element> = Vec::new();
    for (i, e) in stock.elements.iter().enumerate() {
        if !drop_ids.contains(&i) {
            exp.push(e);
        }
    }
    if exp.len() != probe.elements.len() {
        println!(
            "COUNT MISMATCH: expected {} got {} (probe header {})",
            exp.len(),
            probe.elements.len(),
            probe.element_count
        );
    }
    let mut diffs = 0usize;
    for (i, (e, p)) in exp.iter().zip(probe.elements.iter()).enumerate() {
        if e.name != p.name
            || e.x_pau != p.x_pau
            || e.y_pau != p.y_pau
            || e.has_pos != p.has_pos
            || e.belonging != p.belonging
            || e.category != p.category
        {
            diffs += 1;
            if diffs <= 12 {
                println!(
                    "DIFF @{i}: '{}' vs '{}' pos({},{})/({},{}) has{} /{} bel {:#x}/{:#x} cat {}/{}",
                    e.name, p.name, e.x_pau, e.y_pau, p.x_pau, p.y_pau, e.has_pos, p.has_pos,
                    e.belonging, p.belonging, e.category, p.category
                );
            }
        }
    }
    // probe-only elements (the new spellings)
    let extra: Vec<&str> = probe
        .elements
        .iter()
        .map(|e| e.name.as_str())
        .filter(|n| !exp.iter().any(|e| e.name == *n))
        .collect();
    println!(
        "stock {} -> probe {} elems; lost {:?}; extra {extra:?}; attr-diffs {diffs}",
        stock.elements.len(),
        probe.elements.len(),
        lost,
    );
}
