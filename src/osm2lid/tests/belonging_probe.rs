// One-off RE harness (#[ignore]): belonging (0x40c) id space. The device accessors index columns by
// *block-local* element id (CalculateTerminatingElementIndex per block). Check whether stock belonging
// values look per-block (restart each block, < block element count) or file-global.

const LID: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";

#[test]
#[ignore]
fn belonging_domain_probe() {
    for file in ["LID20004.DAT", "LID20006.DAT", "LID20000.DAT"] {
        let bytes = std::fs::read(format!("{LID}{file}")).expect("stock file");
        let b = bytes.as_slice();
        let hdr = u32(b, 0x10) as usize;
        let sec1 = u32(b, hdr + 0x18 + 1 * 5 + 1) as usize;
        let n = b.len();
        let nblocks = u16(b, hdr + 4) as usize; // u32 elem, u32 blocks (stock layout on POL tiles)
        let bs0 = u32(b, sec1) as usize;
        let e0 = u16(b, bs0 + 4) as usize;
        let _ = nblocks;
        let nl = lid_format::read(b).expect("read");
        let mut vals: Vec<(usize, u32, usize)> = Vec::new();
        let mut cum = 0usize;
        for ei in 0..nl.elements.len() {
            // best-effort per-block local index: assume blocks of `e0` elements (stock uses uniform-ish)
            let (blk, local) = (ei / e0.max(1), ei % e0.max(1));
            let bg = nl.elements[ei].belonging;
            if bg != 0xffff_ffff {
                vals.push((blk, bg, local));
            }
        }
        let _ = cum;
        vals.sort_by_key(|v| v.1);
        println!(
            "== {file}: elements={} nblocks~{nblocks} e0={e0} belonging_cnt={}",
            nl.element_count,
            vals.len()
        );
        if !vals.is_empty() {
            for &(blk, v, local) in [&vals[0], &vals[vals.len() / 2], &vals[vals.len() - 1]] {
                let bn = nl
                    .elements
                    .get(v as usize)
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                println!("   blk{blk} local={local} belong={v} (as-GLOBAL name: '{bn}')");
            }
            // how many of the set have belong < (block-local-range end) i.e. plausible local ids?
            let maxv = vals.iter().map(|v| v.1).max().unwrap_or(0);
            println!("   max belong={maxv} vs elements={}", nl.elements.len());
        }
        // print first few name/belong pairs with names at that local slot of same block
        let mut p = 0usize;
        for (i, e) in nl.elements.iter().enumerate() {
            if e.belonging == 0xffff_ffff {
                continue;
            }
            let bn = nl
                .elements
                .get(i.wrapping_sub(e.belonging as usize))
                .map(|x| x.name.as_str())
                .unwrap_or("?");
            let gn = nl
                .elements
                .get(e.belonging as usize)
                .map(|x| x.name.as_str())
                .unwrap_or("?");
            println!(
                "   #{i} '{}' belong={:#x} -> global:'{gn}' (i-belong:'{bn}')",
                e.name, e.belonging
            );
            p += 1;
            if p >= 6 {
                break;
            }
        }
    }
}

fn u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
