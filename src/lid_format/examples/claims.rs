use lid_format::{decode_u16_probe, decode_u32_probe};
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn main() {
    for (nm, path, base) in [
        ("STOCK0", "/tmp/opencode/stock1r.DAT", 1428usize),
        ("OURS0", "trials/28 - LID/raw/LID20001.DAT", 239usize),
    ] {
        let b = std::fs::read(path).unwrap();
        let ne1 = u16le(&b, base) as usize;
        let ndesc = u16le(&b, base + 6) as usize;
        let toc = base + 8;
        let mut rows = Vec::new();
        for i in 0..=ndesc {
            let o = toc + i * 12;
            if o + 12 > b.len() {
                break;
            }
            rows.push((
                u16le(&b, o),
                u16le(&b, o + 2),
                u32le(&b, o + 4),
                u32le(&b, o + 8),
            ));
        }
        let win = |i: usize| -> u32 {
            if i + 1 < rows.len() {
                rows[i + 1].2.wrapping_sub(rows[i].2)
            } else {
                let p = toc + rows.len() * 12 + 8;
                let g = if p + 4 <= b.len() {
                    u32le(&b, p)
                } else {
                    u32::MAX
                };
                g.wrapping_sub(rows[i].2)
            }
        };
        let find = |t: u16| {
            rows.iter()
                .enumerate()
                .find(|(_, r)| r.0 == t)
                .map(|(i, r)| (i, *r))
                .unwrap()
        };
        let dec = |tag: u16, n: usize| -> Vec<u32> {
            let (i, r) = find(tag);
            let (v, _) = decode_u32_probe(
                &b,
                r.1 as u32,
                base + r.2 as usize,
                base + r.2 as usize + win(i) as usize,
                n,
            );
            v
        };
        let (degs, _) = decode_u16_probe(
            &b,
            find(0x401).1 .1 as u32,
            base + find(0x401).1 .2 as usize,
            base + find(0x401).1 .2 as usize + win(find(0x401).0) as usize,
            ne1,
        );
        let nedges: usize = degs.iter().map(|x| *x as usize).sum();
        let claims = dec(0x406, find(0x406).1 .3 as usize);
        let d0 = degs[0] as usize;
        let sum_first_d0: usize = claims[..d0.min(claims.len())]
            .iter()
            .map(|&x| x as usize)
            .sum();
        println!(
            "{nm}: ne1={ne1} nedges={nedges} d0={d0} claim_cnt={} claims_sum={} sum_first{d0}={}",
            claims.len(),
            claims.iter().map(|&x| x as usize).sum::<usize>(),
            sum_first_d0
        );
        println!("  degs[..12]= {:?}", &degs[..d0.min(12)]);
        println!("  claims[12]= {:?}", &claims[..claims.len().min(12)]);
        println!(
            "  claims tail: {:?}",
            &claims[claims.len().saturating_sub(6)..]
        );
    }
}
