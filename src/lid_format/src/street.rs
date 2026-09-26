//! From-scratch STREET name-list (`LID20006`, listID 3) generation + its REL00001 owner matrix.
//!
//! Device facts (all CONFIRMED 2026-09-26, `device_sim::check_block_load` on stock LID20006
//! blocks 0..97 PASS): a street block uses the SAME 37-row attr-vector template as the city
//! list (`city::build_city_block` is shared verbatim, including the dummy last TOC row and the
//! `0x40d` validDestination fix). Only the sub-header differs from the city list:
//! `listID = 3`, `nrel = 0`, `nrec = 0`, sec5 = `[3]` (UI category), sec6 = `[39]` (POL lang id),
//! and its own position origin (stock POL = 168702855/585365864).
//!
//! The street browser fetches a city's streets from REL00001 rows (street = SOURCE side,
//! city = TARGET side), so `build_street_relations` emits that matrix with OUR street ids.

use crate::city::{build_city_block, CityEntry};

/// Street-list element owner matrix: street element id -> owning city element id.
/// Emits `REL00001` shape (listIDs 3->2); the device reads it with a row query on the city.
pub fn build_street_relations(city_of: &[u32], city_count: u32) -> Result<Vec<u8>, String> {
    use crate::rel::write_rel;
    let n_street = city_of.len() as u64;
    let pairs: Vec<(u32, u32)> = city_of
        .iter()
        .enumerate()
        .map(|(i, &c)| (i as u32, c))
        .collect();
    write_rel(n_street, u64::from(city_count), 3, 2, &pairs)
}

fn u32b(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}
fn u16b(v: u16) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// Build a complete street name-list file (`LID20006` shape) from uppercase names + PAU
/// positions. `stock` must be the stock POL `LID20006.DAT` (author block, stream codes,
/// category/language vectors and the position origin are copied from it verbatim).
/// Returns (file bytes, origin) — element ids are TEIC order (byte order of names).
pub fn build_street_file(streets: &[CityEntry], stock: &[u8]) -> (Vec<u8>, (i32, i32)) {
    assert!(!streets.is_empty(), "street list must be non-empty");
    let hdr = 119usize;
    let origin = (
        i32::from_le_bytes(stock[hdr + 16..hdr + 20].try_into().unwrap()),
        i32::from_le_bytes(stock[hdr + 20..hdr + 24].try_into().unwrap()),
    );
    // device subheader layout: +0 elem(+flag), +4 nb, +6 nrec, +8 nrel, +0x0a cnt5, +0x0c cnt6,
    // +0x0e shift — the sec5/sec6 vectors are copied verbatim, so their counts are stock's own.
    let cnt5 = u16::from_le_bytes(stock[hdr + 10..hdr + 12].try_into().unwrap());
    let cnt6 = u16::from_le_bytes(stock[hdr + 12..hdr + 14].try_into().unwrap());
    let shift = u16::from_le_bytes(stock[hdr + 14..hdr + 16].try_into().unwrap());
    let (block, _nbel) = build_city_block(streets, origin);
    let blocks = vec![block];
    let nb = blocks.len();

    // stock's own stream table (7 x {u8 code, u32 off}) at hdr+24, stride 5.
    let tbl_at = hdr + 24;
    let mut codes = [0u8; 7];
    let mut soff = [0u32; 7];
    for i in 0..7 {
        codes[i] = stock[tbl_at + 5 * i];
        soff[i] = u32::from_le_bytes(
            stock[tbl_at + 5 * i + 1..tbl_at + 5 * i + 5]
                .try_into()
                .unwrap(),
        );
    }
    let sec5 = stock[soff[5] as usize..soff[6] as usize].to_vec(); // [3]
    let first_block = u32::from_le_bytes(
        stock[soff[1] as usize..soff[1] as usize + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    let sec6 = stock[soff[6] as usize..first_block].to_vec(); // [39]

    let sec0: Vec<u8> = blocks
        .iter()
        .flat_map(|b| crate::vle_encode(b.len() as u32))
        .collect();
    let sec1_len = 4 * nb;
    let base = tbl_at + 35;
    let layout = [
        sec0.len(),
        sec1_len,
        0usize,
        0usize,
        0usize,
        sec5.len(),
        sec6.len(),
    ];
    let mut o = base;
    let offs_sec: Vec<usize> = layout
        .iter()
        .map(|l| {
            let c = o;
            o += l;
            c
        })
        .collect();
    let streams_end = o;
    let block_abs = streams_end;
    let blob_abs = block_abs + blocks.iter().map(|b| b.len()).sum::<usize>();

    let mut s: Vec<u8> = Vec::with_capacity(blob_abs);
    s.extend_from_slice(&stock[0..hdr]); // author block: kind, listID 3, region, sizes
    s.extend_from_slice(&u32b(streets.len() as u32)); // element_count
    s.extend_from_slice(&u16b(nb as u16)); // blocks
    s.extend_from_slice(&u16b(0)); // nrec
    s.extend_from_slice(&u16b(0)); // nrel (D)
    s.extend_from_slice(&u16b(cnt5)); // cnt5 (sec5 vector entries)
    s.extend_from_slice(&u16b(cnt6)); // cnt6 (sec6 vector entries)
    s.extend_from_slice(&u16b(shift));
    s.extend_from_slice(&stock[hdr + 16..hdr + 24]); // position origin
    assert_eq!(s.len(), tbl_at);
    for (i, c) in codes.iter().enumerate() {
        s.push(*c);
        s.extend_from_slice(&u32b(offs_sec[i] as u32));
    }
    assert_eq!(s.len(), base);
    s.resize(streams_end, 0);
    s[offs_sec[0]..][..sec0.len()].copy_from_slice(&sec0);
    for (i, blk) in blocks.iter().enumerate() {
        s[offs_sec[1] + 4 * i..offs_sec[1] + 4 * i + 4].copy_from_slice(&u32b(
            (block_abs + blocks[..i].iter().map(|b| b.len()).sum::<usize>()) as u32,
        ));
    }
    s[offs_sec[5]..][..sec5.len()].copy_from_slice(&sec5);
    s[offs_sec[6]..][..sec6.len()].copy_from_slice(&sec6);
    for blk in blocks.iter() {
        s.extend_from_slice(blk);
    }
    assert_eq!(s.len(), blob_abs);
    s[20..24].copy_from_slice(&u32b((streams_end - hdr) as u32));
    (s, origin)
}

/// House-number ranges for one street (odd/even `from..=to`, `0,0` = none).
#[derive(Debug, Clone, Copy)]
pub struct HnrRange {
    pub street_elem: u32,
    pub odd: (u32, u32),
    pub even: (u32, u32),
}

/// Build the HNR GenAttr file (`LID40006.DAT`, fileID+20000) for `ranges` over a street list of
/// `street_count` elements. One block, flat records in street order (odd numbers first). Column
/// set = the device HNR read path (`enGetHnrIndices` `00e0c3a0` + `enGetHnr` `00e0d078`):
/// `0xc01` numbers (existence bitmap + cumulative starts + flat values), `0xc02` owner street
/// SV, `0xc11` record gate, `0xc09`/`0xc0a` even/odd parity bits, `0xc0b`/`0xc0d` unused bits,
/// `0xc03..0xc06` empty street/suffix string columns. `outer` = stock LID40006 bytes (author
/// block; hdr pointer is re-derived by the writer).
pub fn build_hnr_file(
    street_count: u32,
    ranges: &[HnrRange],
    outer: &[u8],
) -> Result<Vec<u8>, String> {
    use crate::write::{write_gen_attr_file, BlockData, ColData, ColKind};
    let mut nums: Vec<u32> = Vec::new();
    let mut refs: Vec<u32> = Vec::new();
    for r in ranges {
        for &(a, b) in [r.odd, r.even].iter() {
            let mut n = a;
            while b != 0 && n <= b {
                nums.push(n);
                refs.push(r.street_elem);
                n += 2;
            }
        }
    }
    let nrec = nums.len();
    if nrec == 0 {
        return Err("HNR file needs at least one record".into());
    }
    let span = street_count; // TOC element ranges are INCLUSIVE: the block covers 0..=street_count-1
    let mut exists = vec![false; span as usize];
    let mut starts: Vec<u32> = Vec::new();
    let mut pos = 0u32;
    for r in ranges {
        let cnt = refs.iter().filter(|&&x| x == r.street_elem).count() as u32;
        if cnt != 0 {
            exists[r.street_elem as usize] = true;
            starts.push(pos);
            pos += cnt;
        }
    }
    let col = |selector: u16,
               kind: ColKind,
               domain: u32,
               exists: Vec<bool>,
               counts: Vec<u32>,
               values: Vec<u32>,
               bits: Vec<bool>| ColData {
        selector,
        kind,
        domain,
        exists,
        counts,
        values,
        bits,
        code_8000: 0x16,
        code_0000: 0x14,
        range_from_to: None,
    };
    let empty = |selector: u16| ColData {
        selector,
        kind: ColKind::EmptyByteList,
        domain: nrec as u32,
        exists: vec![],
        counts: vec![],
        values: vec![],
        bits: vec![],
        code_8000: 0x11,
        code_0000: 0x11,
        range_from_to: None,
    };
    let even_bits: Vec<bool> = nums.iter().map(|&n| n % 2 == 0).collect();
    let odd_bits: Vec<bool> = nums.iter().map(|&n| n % 2 == 1).collect();
    let file = write_gen_attr_file(
        street_count,
        outer,
        &[BlockData {
            elem_start: 0,
            elem_end: street_count - 1,
            cols: vec![
                col(
                    0xc01,
                    ColKind::ValueList,
                    span,
                    exists,
                    starts,
                    nums,
                    vec![],
                ),
                col(
                    0xc02,
                    ColKind::SingleValue,
                    nrec as u32,
                    vec![],
                    vec![],
                    refs,
                    vec![],
                ),
                col(
                    0xc11,
                    ColKind::ValueList,
                    nrec as u32,
                    vec![true; nrec],
                    (0..nrec as u32).collect(),
                    (0..nrec as u32).collect(),
                    vec![],
                ),
                col(
                    0xc09,
                    ColKind::Binary,
                    nrec as u32,
                    vec![],
                    vec![],
                    vec![],
                    even_bits,
                ),
                col(
                    0xc0a,
                    ColKind::Binary,
                    nrec as u32,
                    vec![],
                    vec![],
                    vec![],
                    odd_bits,
                ),
                col(
                    0xc0b,
                    ColKind::Binary,
                    nrec as u32,
                    vec![],
                    vec![],
                    vec![],
                    vec![false; nrec],
                ),
                col(
                    0xc0d,
                    ColKind::Binary,
                    nrec as u32,
                    vec![],
                    vec![],
                    vec![],
                    vec![false; nrec],
                ),
                empty(0xc03),
                empty(0xc04),
                empty(0xc05),
                empty(0xc06),
            ],
        }],
    );
    Ok(file)
}
