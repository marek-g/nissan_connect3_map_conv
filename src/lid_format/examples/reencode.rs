// Re-encode every stock block with our build_block and compare bytes.
// Usage: reencode <LIDfile> [max_blocks]
use lid_format::{build_block, read, NameEntry};

fn u32at(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}
fn u16at(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes(b[p..p + 2].try_into().unwrap())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let b = std::fs::read(&args[1]).unwrap();
    let maxb: usize = args
        .get(2)
        .map(|x| x.parse().unwrap())
        .unwrap_or(usize::MAX);
    let nl = read(&b).unwrap();
    let hdr = u32at(&b, 0x10) as usize;
    let nblk = u16at(&b, hdr + 4) as usize;
    let sec1 = u32at(&b, hdr + 24 + 5 + 1) as usize;
    let sec3 = u32at(&b, hdr + 24 + 3 * 5 + 1) as usize;
    let nrec = u16at(&b, hdr + 6) as usize;
    let boff: Vec<usize> = (0..nblk)
        .map(|i| u32at(&b, sec1 + 4 * i) as usize)
        .collect();
    let tail_min = (0..nrec)
        .map(|i| u32at(&b, sec3 + 4 * i) as usize)
        .filter(|&v| v >= boff[0])
        .min()
        .unwrap_or(b.len());
    let mut same = 0usize;
    let mut diff = 0usize;
    let mut by_block: Vec<Vec<&lid_format::Element>> = vec![Vec::new(); nl.block_count];
    for e in &nl.elements {
        by_block[e.block].push(e);
    }
    for (bi, elems) in by_block.iter().enumerate() {
        if bi >= maxb {
            break;
        }
        let be = if bi + 1 < nblk {
            boff[bi + 1]
        } else {
            tail_min.min(b.len())
        };
        let orig = &b[boff[bi]..be];
        let entries: Vec<NameEntry> = elems
            .iter()
            .map(|e| NameEntry {
                label: e.sort_name.clone(),
                x_pau: e.x_pau,
                y_pau: e.y_pau,
                city: Some((0, 0)),
                belonging: (e.belonging != 0xffffffff).then_some(e.belonging),
            })
            .collect();
        let anchor = if elems.iter().any(|e| e.has_pos) {
            Some((0, 0))
        } else {
            None
        };
        let (mine, _) = build_block(anchor, &entries);
        if mine == orig {
            same += 1;
        } else {
            diff += 1;
            let fd = mine.iter().zip(orig.iter()).position(|(x, y)| x != y);
            println!(
                "block {bi}: DIFF elems={} origlen={} minelen={} firstdiff={:?}",
                elems.len(),
                orig.len(),
                mine.len(),
                fd
            );
        }
    }
    println!("SAME={same} DIFF={diff} (of {} tested)", same + diff);
}
