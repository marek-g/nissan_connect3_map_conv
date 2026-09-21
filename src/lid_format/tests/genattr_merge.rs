use lid_format::write::{write_gen_attr_file, BlockData, ColData, ColKind};

fn file(elem_count: u32, blocks: &[(u32, u32, Vec<u32>)]) -> Vec<u8> {
    let bds = blocks
        .iter()
        .map(|&(s, e, ref vals)| BlockData {
            elem_start: s,
            elem_end: e,
            cols: vec![ColData {
                selector: 0x001,
                domain: 0,
                kind: ColKind::Simple32,
                code_0000: 0x11,
                code_8000: 0,
                exists: vec![],
                counts: vec![],
                values: vals.clone(),
                bits: vec![],
                range_from_to: None,
            }],
        })
        .collect::<Vec<_>>();
    write_gen_attr_file(elem_count, &[], &bds)
}

fn toc(f: &[u8]) -> (u32, Vec<(usize, u32, u32)>) {
    let u = |o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap()) as usize;
    let hdr = u(0x10);
    let n = u(hdr + 8);
    let e = (0..n)
        .map(|i| {
            (
                u(hdr + 12 + 12 * i),
                u(hdr + 16 + 12 * i) as u32,
                u(hdr + 20 + 12 * i) as u32,
            )
        })
        .collect();
    (u(hdr) as u32, e)
}

fn payload(f: &[u8], off: usize, end: usize) -> Vec<u8> {
    f[off..end].to_vec()
}

#[test]
fn splice_appends_and_preserves() {
    let stock = file(8, &[(0, 8, vec![10, 11, 12, 13, 14, 15, 16, 17])]);
    let our = file(12, &[(8, 12, vec![20, 21, 22, 23])]);
    let m = lid_format::write::merge_gen_attr(&stock, &our).unwrap();

    let (se, stoc) = toc(&stock);
    let (oe, otoc) = toc(&our);
    let (me, mtoc) = toc(&m);
    assert_eq!(me, oe.max(se));
    assert_eq!(mtoc.len(), stoc.len() + otoc.len());
    assert_eq!(mtoc[0].1, 0);
    assert_eq!(mtoc[1], (mtoc[0].0 + payload(&stock, stoc[0].0, stock.len()).len(), 8, 12));
    let s0 = payload(&stock, stoc[0].0, stock.len());
    assert_eq!(payload(&m, mtoc[0].0, mtoc[1].0), s0);
    let o0 = payload(&our, otoc[0].0, our.len());
    assert_eq!(payload(&m, mtoc[1].0, m.len()), o0);
    assert!(mtoc[0].0 >= 0x10 + 16 + 12 * 2);
}

#[test]
fn splice_rejects_overlap() {
    let stock = file(8, &[(0, 8, vec![1; 8])]);
    let our = file(8, &[(4, 8, vec![1; 4])]);
    assert!(lid_format::write::merge_gen_attr(&stock, &our).is_err());
}
