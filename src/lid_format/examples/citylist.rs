// usage: citylist            -> built-in scenarios (stock family + our files in /tmp/opencode)
use lid_format::device_sim::check_city_list;

const POL: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                   CRYPTNAV/DATA/DATA/LID/CCP/POL";

fn rd(p: &str) -> Vec<u8> {
    std::fs::read(p).unwrap_or_else(|e| panic!("read {p}: {e}"))
}

fn report(tag: &str, lid: &[u8], rels: &[(&[u8], bool)], ctx: &[(u32, u32)]) {
    let owned: Vec<(u16, &[u8], bool)> = rels
        .iter()
        .enumerate()
        .map(|(i, (b, bs))| (i as u16, *b, *bs))
        .collect();
    match check_city_list(lid, &owned, ctx) {
        Ok(r) => {
            let mut hist: std::collections::BTreeMap<&str, usize> = Default::default();
            for (_, why) in &r.dropped {
                *hist.entry(*why).or_default() += 1;
            }
            println!(
                "{tag}: ne={} keys={} candidates={} LIST={} dropped={:?} danger={}",
                r.ne,
                r.keys,
                r.candidates,
                r.list.len(),
                hist,
                r.danger.len()
            );
            if r.list.len() <= 12 {
                println!("   list={:?}", r.list);
            }
        }
        Err(g) => println!("{tag}: FAIL {}", g.0),
    }
}

fn main() {
    // Scenario A: stock family, full context (collector validation: expect huge non-empty list)
    let sl = rd(&format!("{POL}/LID20001.DAT"));
    let r0 = rd(&format!("{POL}/REL00000.DAT"));
    let r1 = rd(&format!("{POL}/REL00001.DAT"));
    let r3 = rd(&format!("{POL}/REL00003.DAT"));
    let r6 = rd(&format!("{POL}/REL00006.DAT"));
    report(
        "STOCK full ctx",
        &sl,
        &[(&r0, true), (&r1, false), (&r3, false), (&r6, false)],
        &[(0, 44661)],
    );

    // Scenario B: stock family, context = stock ids of the 10 test cities (gates per city)
    let ids: Vec<u32> = vec![
        194288, 199864, 208837, 211986, 212340, 222141, 222146, 223516, 233759, 236833,
    ];
    let ctx: Vec<(u32, u32)> = ids.iter().map(|&i| (i, i + 1)).collect();
    report(
        "STOCK 10-city ctx",
        &sl,
        &[(&r0, true), (&r1, false), (&r3, false), (&r6, false)],
        &ctx,
    );

    // Scenario C: our NEW family (post 0x40d fix), context = all elements
    let tl = rd("/tmp/opencode/zzcity10.DAT");
    let t0 = rd("/tmp/opencode/REL00000.DAT");
    let t1 = rd("/tmp/opencode/REL00001.DAT");
    let t3 = rd("/tmp/opencode/REL00003.DAT");
    let t6 = rd("/tmp/opencode/REL00006.DAT");
    report(
        "NEW family",
        &tl,
        &[(&t0, true), (&t1, false), (&t3, false), (&t6, false)],
        &[(0, 10)],
    );

    // Scenario D: OLD card build (broken 0x40d) - deterministic-in-sim list but danger flagged;
    // on the card the validDestination bits came from heap past the block buffer (random clears).
    let ol = rd("/tmp/opencode/old_city.DAT");
    report(
        "OLD card LID + new RELs",
        &ol,
        &[(&t0, true), (&t1, false), (&t3, false), (&t6, false)],
        &[(0, 10)],
    );
}
