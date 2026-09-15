// One-off RE harness (#[ignore]): pin the name-list position decode + ORIGIN.
//
// Sub-header (read u32 @0x10 = sub-header start `hdr`):
//   hdr+0x00 u32 elem_count | +0x04 u32 block_count | +0x08 u32 (0) | +0x0c u32 flags (0x80000 set => origin valid)
//   hdr+0x10 u32 origin X (PAU) | hdr+0x14 u32 origin Y (PAU)   (tNLHPosition; -1/-1 => file carries no positions)
//   hdr+0x18 7x{u8 code,u32 off} sections (sec1 = raw u32 block file-offsets).
// Ghidra: NLNameList::GetAsfSubHeader=this+0x7c; NLAsfBlock::enSetListDescriptions(@00cdc0c0) builds
// {off, len=off_next-off_this, param, kindWithFlags, code}; NLPositionAttrVector::SetDescription
// (@00cdb458): flags 0x4000 => slot +0 (existence bitmap), flags 0 => slot +0x10 (coord stream);
// NLPositionAttrVector::Decode (@00cdd7dc): bitmap over elem_count, then 2*popcount u32 coords stored
// in vector this+0x30, origin copied verbatim to this+0x48/0x4c.
//
// Question probed here: do X/Y live ONLY in the 0x407 coord stream, and is (X,Y) interleaved or
// delta-chained? Validation = bbox of reconstructed abs positions must cover the tile's country area,
// and known street anchors must land within a few km of truth.

const LID: &str = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/CRYPTNAV/DATA/DATA/LID/CCP/POL/";
const PAU: f64 = (1i64 << 31) as f64 / 180.0;
// Poland + margin (deg): lon [13.5,24.5], lat [48.5,55.0]
fn in_pl(lon: f64, lat: f64) -> bool {
    (13.0..=25.5).contains(&lon) && (48.0..=55.5).contains(&lat)
}

#[test]
#[ignore]
fn position_streams_probe() {
    for file in ["LID20004.DAT", "LID20006.DAT", "LID20000.DAT"] {
        let bytes = std::fs::read(format!("{LID}{file}")).expect("stock file");
        probe_file(file, &bytes);
    }
}

// (folded street-name substring, truth lon, lat) — streets unique to one city.
const ANCHORS: [(&str, f64, f64); 8] = [
    ("JEROZOLIMSK", 21.0010, 52.2213),  // Warszawa, Aleje Jerozolimskie
    ("MARSZALKOWSK", 21.0100, 52.2290), // Warszawa
    ("NOWY SWIAT", 21.0120, 52.2326),   // Warszawa
    ("TUMSKA", 21.0190, 52.2420),       // Warszawa
    ("PIOTROWSK", 19.9400, 50.0530),    // Krakow
    ("KRUPPWKI", 19.9807, 49.2960),     // Zakopane
    ("SWIETOKRZYSKA", 20.6230, 50.8720), // Kielce
    ("LIPSKA", 19.4560, 51.6740),       // Lodz
];

#[test]
#[ignore]
fn anchor_fit_with_current_lib() {
    for file in ["LID20004.DAT", "LID20006.DAT"] {
        fit_file(file);
    }
}

fn fit_file(file: &str) {
    let bytes = std::fs::read(format!("{LID}{file}")).expect("stock file");
    let b = bytes.as_slice();
    let nl = lid_format::read(b).expect("read");
    let (ox, oy) = nl.origin.unwrap_or((0, 0));
    // block -> element-range boundaries (u16 elem_count at block_off+4)
    let hdr = u32(b, 0x10) as usize;
    let sec1 = u32(b, hdr + 0x18 + 6) as usize;
    let block_count = ((u32(b, hdr + 0x18 + 3 * 5) as usize - sec1) / 4).min(1024);
    let n = b.len();
    let mut elem_to_block: Vec<u16> = Vec::with_capacity(nl.elements.len());
    for bi in 0..block_count {
        let bs = u32(b, sec1 + bi * 4) as usize;
        if bs + 8 > n {
            break;
        }
        let e = u16(b, bs + 4) as usize;
        elem_to_block.resize(elem_to_block.len() + e, bi as u16);
    }
    // section table: 7x{u8 code, u32 off} at hdr+0x18
    let mut sec = [(0u32, 0u32); 7];
    for i in 0..7 {
        sec[i] = (b[hdr + 0x18 + i * 5] as u32, u32(b, hdr + 0x18 + i * 5 + 1));
    }
    println!(
        "== {file}: block_count={block_count} origin=({ox},{oy}) secs={}",
        sec.iter()
            .map(|(c, o)| format!("[{c:#03x}@{o:#x}]"))
            .collect::<Vec<_>>()
            .join("")
    );
    for (cnti, cnt) in [
        ("u16@hdr+0", u16(b, hdr) as usize),
        ("u16@hdr+2", u16(b, hdr + 2) as usize),
        ("u16@hdr+4", u16(b, hdr + 4) as usize),
        ("u16@hdr+6", u16(b, hdr + 6) as usize),
        ("u16@hdr+8", u16(b, hdr + 8) as usize),
    ] {
        println!("   {cnti}={cnt}");
    }
    // decode candidate per-block vectors with the several plausible counts
    let mut vecs: Vec<Vec<u32>> = Vec::new();
    for &(si, cnt) in &[
        (2usize, u16(b, hdr + 2) as usize),
        (2, u16(b, hdr + 4) as usize),
        (3, u16(b, hdr + 2) as usize),
        (3, u16(b, hdr + 4) as usize),
        (0, u16(b, hdr + 4) as usize),
    ] {
        let end = n;
        let (v, _) = lid_format::decode_u32_probe(b, sec[si].0, sec[si].1 as usize, end, cnt);
        println!(
            "   sec{si} cnt{cnt} code={:#03x} first8={:?}",
            sec[si].0,
            &v[..v.len().min(8)]
        );
        vecs.push(v);
    }
    // per-anchor per-block residuals
    let km = |v: i64| v as f64 * 111.32 / PAU;
    println!(
        "   elem_to_block len={} elements={}",
        elem_to_block.len(),
        nl.elements.len()
    );
    let mut seen = std::collections::HashMap::<(String, u16), (i64, i64)>::new();
    for (pat, lon, lat) in ANCHORS {
        let p = fold(pat);
        for (i, e) in nl
            .elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.has_pos && fold(&e.name).contains(&p))
        {
            if i >= elem_to_block.len() {
                continue;
            }
            let blk = elem_to_block[i];
            let err = (
                (lon * PAU) as i64 - ox as i64 - e.x_pau as i64,
                (lat * PAU) as i64 - oy as i64 - e.y_pau as i64,
            );
            *seen.entry((pat.to_string(), blk)).or_insert(err) = err;
        }
    }
    let mut rows: Vec<(String, u16, i64, i64)> = seen
        .into_iter()
        .map(|((p, blk), (ex, ey))| (p, blk, ex, ey))
        .collect();
    rows.sort();
    for (p, blk, ex, ey) in rows.iter().take(10) {
        println!("   {p} blk{blk}: err=({:+.0},{:+.0})km", km(*ex), km(*ey));
    }
    // implied city centre (avg) per anchor, then locate its u32 pair anywhere in the file
    let mut city: std::collections::HashMap<String, (i64, i64)> = std::collections::HashMap::new();
    for (pat, lon, lat) in ANCHORS {
        let p = fold(pat);
        let mut acc = (0i64, 0i64);
        let mut cnt = 0i64;
        for e in nl
            .elements
            .iter()
            .filter(|e| e.has_pos && fold(&e.name).contains(&p))
        {
            // stored deltas are pos-city => city ~= truth - delta(=x_pau)
            acc.0 += (lon * PAU) as i64 - e.x_pau as i64 - ox as i64;
            acc.1 += (lat * PAU) as i64 - e.y_pau as i64 - oy as i64;
            cnt += 1;
        }
        if cnt > 0 {
            city.insert(
                pat.to_string(),
                (acc.0 / cnt + ox as i64, acc.1 / cnt + oy as i64),
            );
        }
    }
    for (p, (cx, cy)) in &city {
        let mut hits = Vec::new();
        for off in (0..n - 8).step_by(4) {
            if u32(b, off) as i64 - cx < 0 {
                continue;
            }
            let dx = (u32(b, off) as i64 - cx).abs();
            let dy = (u32(b, off + 4) as i64 - cy).abs();
            if dx < cx / 500 && dy < cy / 500 {
                hits.push((off, dx, dy))
            }
        }
        // also try (Y,X) order and +origin-relative encodings
        println!(
            "   city {p} ({},{}) = ({:.4},{:.4}) hits(X,Y)={:?}",
            cx,
            cy,
            *cx as f64 / PAU,
            *cy as f64 / PAU,
            &hits[..hits.len().min(5)]
        );
    }
}

fn fold(s: &str) -> String {
    s.to_uppercase()
        .chars()
        .map(|c| match c {
            'Ą' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
            'Ć' | 'Ç' | 'Č' => 'C',
            'Ę' | 'È' | 'É' | 'Ê' | 'Ë' => 'E',
            'Ł' => 'L',
            'Ń' | 'Ñ' => 'N',
            'Ó' | 'Ò' | 'Ô' | 'Õ' | 'Ö' | 'Ø' => 'O',
            'Ś' | 'Š' | 'Ș' => 'S',
            'Ź' | 'Ż' | 'Ž' => 'Z',
            'Ü' | 'Û' | 'Ù' | 'ÿ' => 'U',
            'Ý' => 'Y',
            c if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
            c => c,
        })
        .collect()
}

fn probe_file(file: &str, b: &[u8]) {
    let hdr = u32(b, 0x10) as usize;
    let flags = u32(b, hdr + 0x0c);
    let ox = u32(b, hdr + 0x10) as i32;
    let oy = u32(b, hdr + 0x14) as i32;
    let sec1 = u32(b, hdr + 0x18 + 1 * 5 + 1) as usize; // 7x{u8 code,u32 off}: entry1 off
    let block_count = u32(b, hdr + 4) as usize;
    let n = b.len();
    println!(
        "== {file}: n={n} flags={flags:#08x} origin=({ox},{oy}) = ({:.4},{:.4}) sec1@{sec1:#x}",
        ox as f64 / PAU,
        oy as f64 / PAU
    );

    let mut offs = Vec::with_capacity(block_count);
    for i in 0..block_count.min(600) {
        offs.push(u32(b, sec1 + i * 4) as usize);
    }

    let mut tot = 0usize;
    let mut in_pl_a = 0usize;
    let mut in_pl_c = 0usize;
    let mut dumped = 0usize;
    for bi in 0..offs.len() {
        let bs = offs[bi];
        let be = if bi + 1 < offs.len() { offs[bi + 1] } else { n };
        if bs + 8 > n {
            break;
        }
        let elem_count_b = u16(b, bs + 4) as usize;
        let num_desc = u16(b, bs + 6) as usize;
        let mut d: Vec<(u32, u32, u32, u32, u32)> = Vec::new(); // kind,flags,code,off,param
        let mut p = bs + 8;
        for _ in 0..num_desc {
            if p + 12 > be {
                break;
            }
            let k = u16(b, p) as u32;
            d.push((
                k & 0xfff,
                k & 0xf000,
                u16(b, p + 2) as u32,
                u32(b, p + 4),
                u32(b, p + 8),
            ));
            p += 12;
        }
        let lenof = |i: usize| {
            if i + 1 < d.len() {
                (d[i + 1].3 as usize).saturating_sub(d[i].3 as usize)
            } else {
                be - bs
            }
        };
        let r407: Vec<usize> = d
            .iter()
            .enumerate()
            .filter(|(_, x)| x.0 == 0x407)
            .map(|(i, _)| i)
            .collect();
        if r407.is_empty() || elem_count_b == 0 {
            continue;
        }
        if dumped < 3 {
            for &i in &r407 {
                let (k, f, c, o, pm) = d[i];
                println!("  blk{bi} e={elem_count_b} row{i}: {k:#x} flags={f:#x} code={c:#03x} off={o:#x} len={:#x} param={pm}", lenof(i));
            }
            dumped += 1;
        }
        // bitmap row = flags 0x4000 (or first), coords row = flags 0
        let brows = d
            .iter()
            .enumerate()
            .find(|(_, x)| x.0 == 0x407 && x.1 == 0x4000)
            .map(|(i, _)| i);
        let crows = d
            .iter()
            .enumerate()
            .find(|(_, x)| x.0 == 0x407 && x.1 == 0)
            .map(|(i, _)| i);
        let Some(ci) = crows else { continue };
        let (bcode, boff, blen, bparam) = match brows {
            Some(bi2) => (d[bi2].2, d[bi2].3 as usize, lenof(bi2), d[bi2].4),
            None => (0, 0, 0, 0),
        };
        let (ccode, coff, clen) = (d[ci].2, d[ci].3 as usize, lenof(ci));
        // bitmap: device caps by len; also probe self-delimited cursor
        let (bmap, _) =
            lid_format::bitfield_probe(b, bcode, bs + boff, bs + boff + blen, elem_count_b);
        let (bmap_full, cur_full) = if brows.is_some() {
            lid_format::bitfield_probe(b, bcode, bs + boff, be, elem_count_b)
        } else {
            (vec![true; elem_count_b], bs + boff)
        };
        let nset = bmap.iter().filter(|x| **x).count();
        let nset_f = if brows.is_some() { nset } else { elem_count_b };
        // variant A: coords at own off, span-capped (current lib rule)
        let (va, _) = lid_format::decode_u32_probe(
            b,
            ccode,
            bs + coff,
            bs + coff + clen,
            2 * nset_f.min(elem_count_b),
        );
        // variant C: coords starting right AFTER the self-delimited bitmap
        let (vc, _) = lid_format::decode_u32_probe(
            b,
            ccode,
            cur_full.max(bs + coff),
            be,
            2 * nset_f.min(elem_count_b),
        );
        let count_inpl = |v: &[u32]| -> usize {
            let mut r = 0usize;
            let mut rank = 0usize;
            for e in 0..elem_count_b {
                let hp = if brows.is_some() { bmap[e] } else { true };
                if !hp || rank * 2 + 1 >= v.len() {
                    continue;
                }
                rank += 1;
                let lon = (ox as i64 + v[rank * 2 - 2] as i64) as f64 / PAU;
                let lat = (oy as i64 + v[rank * 2 - 1] as i64) as f64 / PAU;
                if in_pl(lon, lat) {
                    r += 1
                }
            }
            r
        };
        let ta = count_inpl(&va);
        let tc = count_inpl(&vc);
        tot += nset_f.min(elem_count_b);
        in_pl_a += ta;
        in_pl_c += tc;
        if bi == 0 || (bi < 4 && dumped <= 3) {
            println!("  blk{bi}: bcode={bcode:#03x} blen={blen:#x} bparam={bparam} nset={nset}/{nset_f} ccode={ccode:#03x} coff={coff:#x} clen={clen:#x} :: A_inPL={ta} C_inPL={tc} vA[..6]={:?} vC[..6]={:?}",
                &va[..va.len().min(6)], &vc[..vc.len().min(6)]);
        }
    }
    println!("  TOTAL pos={tot}  variantA inPL={in_pl_a}  variantC(after-bitmap) inPL={in_pl_c}");
    println!();
}

fn u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn u16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
