// Generate a minimal from-scratch LID (city list) file with ONE city.
// Field-by-field semantics ported from NLNameList::LoadHeader + NLAsfBlock/SetDataBlock
// decoders (DAPIAPP.OUT). Validate with diag/LID/devsim.py and lid_format::read().
use lid_format::Element;

fn vle(v: u32) -> Vec<u8> {
    let mut b = Vec::new();
    let mut x = v;
    loop {
        let part = (x & 0x7f) as u8;
        x >>= 7;
        if x == 0 {
            b.push(part);
            break;
        }
        b.push(part | 0x80);
    }
    b
}
fn u16b(v: u16) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}
fn u32b(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}
fn row(kind: u16, code: u16, param: u32, off: u32) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&u16b(kind));
    v.extend_from_slice(&u16b(code));
    v.extend_from_slice(&u32b(off));
    v.extend_from_slice(&u32b(param));
    v
}

const PAU: f64 = (1i64 << 31) as f64 / 180.0;

pub struct MinCity {
    pub name: &'static str,
    pub lon_deg: f64,
    pub lat_deg: f64,
}

pub fn build_min_lid(cities: &[MinCity], stock: &[u8]) -> Vec<u8> {
    // ---- trie: shared root node 0; each city = chain of single-char edges (ASCII only) ----
    let mut od: Vec<u32> = vec![0];
    let mut blob: Vec<u8> = Vec::new();
    let mut loffs: Vec<u32> = Vec::new();
    let nbel = cities.len();
    for c in cities {
        let mut cur = 0usize;
        for ch in c.name.bytes() {
            od.push(0);
            od[cur] += 1;
            cur = od.len() - 1;
            loffs.push(blob.len() as u32); // label start (device: [off[e], off[e+1]))
            blob.push(ch);
        }
    }
    let nc = od.len() as u32;
    let ne = loffs.len() as u32;
    let od_bytes: Vec<u8> = od.iter().flat_map(|&d| vle(d)).collect();
    let claim_bytes: Vec<u8> = (0..ne).flat_map(|_| vle(1)).collect();
    let loff_bytes: Vec<u8> = loffs.iter().flat_map(|&o| vle(o)).collect();
    let mut pos_bytes = Vec::new();
    for c in cities {
        pos_bytes.extend_from_slice(&u32b((c.lon_deg * PAU) as i32 as u32));
        pos_bytes.extend_from_slice(&u32b((c.lat_deg * PAU) as i32 as u32));
    }
    let bin_bytes: Vec<u8> = (0..nbel)
        .flat_map(|_| 0x9000_0000u32.to_le_bytes().to_vec())
        .collect(); // 0x8415 values: S9 tag9 = 1 value, 28 bits, value 0

    // 36 rows in stock block45 order. All kinds present because runtime opts vary per UI path
    // (bVerifyList={0x408,0x409}, vCollectNamesOfCat={0x40c,0x40b}, NLProcessor default=ALL-1),
    // so every column must decode cleanly under any opts.
    let rws: Vec<(u16, u16, u32, Vec<u8>)> = vec![
        (0x4402, 0x02, nc, vec![]),
        (0x0402, 0x14, 0, vec![]),
        (0x0401, 0x14, nc, od_bytes),
        (0x4404, 0x02, ne, vec![]),
        (0x8404, 0x11, 0, vec![]),
        (0x0404, 0x11, 0, vec![]),
        (0x4405, 0x02, ne, vec![]),
        (0x8405, 0x11, 0, vec![]),
        (0x0405, 0x11, 0, vec![]),
        (0x8403, 0x14, ne, loff_bytes),
        (0x0403, 0x11, blob.len() as u32, blob),
        (0x0406, 0x14, ne, claim_bytes),
        (0x0413, 0x02, ne, vec![]),
        (0x4407, 0x03, nbel as u32, vec![]),
        (0x0407, 0x11, (nbel * 2) as u32, pos_bytes),
        (0x440e, 0x02, nbel as u32, vec![]),
        (0x040e, 0x11, 0, vec![]),
        (0x4408, 0x02, nbel as u32, vec![]),
        (0x0408, 0x11, 0, vec![]),
        (0x4409, 0x02, nbel as u32, vec![]),
        (0x8409, 0x11, 0, vec![]),
        (0x0409, 0x11, 0, vec![]),
        (0x440a, 0x02, nbel as u32, vec![]),
        (0x040a, 0x11, 0, vec![]),
        (0x440b, 0x02, nbel as u32, vec![]),
        (0x040b, 0x11, 0, vec![]),
        (0x440c, 0x02, nbel as u32, vec![]),
        (0x040c, 0x11, 0, vec![]),
        (0x8415, 0x18, nbel as u32, bin_bytes),
        (0x0415, 0x01, 0, vec![]),
        (0x040f, 0x02, nbel as u32, vec![]),
        (0x0410, 0x02, nbel as u32, vec![]),
        (0x0411, 0x02, nbel as u32, vec![]),
        (0x0414, 0x02, nbel as u32, vec![]),
        (0x0412, 0x02, nbel as u32, vec![]),
        (0x040d, 0x03, nbel as u32, vec![]),
    ];
    let data_base = 8 + 12 * rws.len();
    let mut data = Vec::new();
    let mut offs = Vec::new();
    for (_, _, _, p) in rws.iter() {
        offs.push((data_base + data.len()) as u32);
        data.extend_from_slice(p);
    }
    let mut block = Vec::new();
    block.extend_from_slice(&u16b(od.len() as u16));
    let sum_od: usize = od.iter().map(|&d| d as usize).sum();
    block.extend_from_slice(&u16b((od.len() - sum_od) as u16)); // +2: forest roots = nc - sum(od)
    block.extend_from_slice(&u16b(nbel as u16));
    block.extend_from_slice(&u16b(rws.len() as u16));
    for i in 0..rws.len() {
        let (k, c, p, _) = rws[i];
        block.extend_from_slice(&row(k, c, p, offs[i]));
    }
    block.extend_from_slice(&data);

    // ---- file layout (two-pass offsets) ----
    // streams copied from stock verbatim: equivalence table (VLE,4), languages (VLE,3),
    // categories (S9 u16,29)
    let sec4 = stock[1393..1397].to_vec();
    let sec5 = stock[1397..1400].to_vec();
    let sec6 = stock[1400..1428].to_vec();
    let sec0 = vle(block.len() as u32);
    let hdr = 119usize;
    let tbl_at = hdr + 24; // sub-header is 24 bytes at hdr
    let base = tbl_at + 35;
    let layout = [sec0.len(), 4, 0, 0, sec4.len(), sec5.len(), sec6.len()];
    let mut o = base;
    let offs_sec: Vec<usize> = layout.iter().map(|l| {
        let c = o;
        o += l;
        c
    })
    .collect();
    let streams_end = o;
    let block_abs = streams_end;

    let mut s: Vec<u8> = Vec::with_capacity(streams_end + block.len());
    // region [0..38):
    s.extend_from_slice(&u16b(0x0402)); // +0  magic/format (this+0x60)
    s.extend_from_slice(&u16b(2)); //     +2  listID = 2 (city list; drives UI category filter)
    s.extend_from_slice(&u16b(0)); //     +4  (this+0x64, stock 0)
    s.extend_from_slice(&u16b(0x41ec)); //+6  (this+0x66, stock const)
    s.extend_from_slice(&u32b(0)); //     +8  (this+0x68)
    s.extend_from_slice(&u32b(1)); //     +12 (this+0x6c)
    s.extend_from_slice(&u32b(hdr as u32)); // +16 header offset (this+0x70)
    s.extend_from_slice(&u32b(0)); //     +20 header size (patched: this+0x74)
    s.extend_from_slice(&u16b(38)); //    +24 metadata string offsets (region copied verbatim)
    s.extend_from_slice(&u16b(84));
    s.extend_from_slice(&u16b(95));
    s.extend_from_slice(&u16b(96));
    s.extend_from_slice(&u16b(13)); //    +32 (this+0x18/0x1c pair, stock values)
    s.extend_from_slice(&u16b(2));
    s.extend_from_slice(&u16b(118)); //   +36 last string offset
    // [38..119): copyright / date / TPLID_EQUIVALENT_CHAR metadata (stock verbatim)
    s.extend_from_slice(&stock[38..119]);
    assert_eq!(s.len(), hdr);
    // sub-header @hdr (this+0x7c .. GetAsfSubHeader):
    s.extend_from_slice(&u32b(nbel as u32)); // +0  global element count (this+0x7c)
    s.extend_from_slice(&u16b(1)); //           +4  block count (this+0x80)
    s.extend_from_slice(&u16b(0)); //            +6  nrec (this+0x84)
    s.extend_from_slice(&u16b(4)); //            +8  sec4 value count (this+0x88)
    s.extend_from_slice(&u16b(3)); //           +10  sec5 value count / languages (this+0x8c)
    s.extend_from_slice(&u16b(29)); //          +12  sec6 value count / categories (this+0x8e)
    s.extend_from_slice(&u16b(8)); //           +14  (this+0x90, stock=8)
    s.extend_from_slice(&stock[hdr + 16..hdr + 24]); // +16 two u32 file-spec constants (stock)
    assert_eq!(s.len(), tbl_at);
    let codes: [u8; 7] = [0x14, 0x11, 0x14, 0x11, 0x14, 0x14, 0x18];
    for (i, c) in codes.iter().enumerate() {
        s.push(*c);
        s.extend_from_slice(&u32b(offs_sec[i] as u32));
    }
    assert_eq!(s.len(), base);
    s.resize(streams_end, 0);
    s[offs_sec[0]..][..sec0.len()].copy_from_slice(&sec0);
    s[offs_sec[1]..offs_sec[1] + 4].copy_from_slice(&u32b(block_abs as u32));
    s[offs_sec[4]..][..sec4.len()].copy_from_slice(&sec4);
    s[offs_sec[5]..][..sec5.len()].copy_from_slice(&sec5);
    s[offs_sec[6]..][..sec6.len()].copy_from_slice(&sec6);
    s.extend_from_slice(&block);
    s[20..24].copy_from_slice(&u32b((streams_end - hdr) as u32)); // this+0x70+this+0x74 = streams_end
    s
}

fn verify(list: &[Element]) {
    for e in list {
        println!("elem block={} node={} name={}", e.block, e.node, e.name);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let stock_path = "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
                      CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20001.DAT";
    let out_path = args
        .get(1)
        .cloned()
        .unwrap_or("/tmp/opencode/zzmin1.DAT".into());
    let stock = std::fs::read(stock_path).expect("read stock");
    let cities = [MinCity {
        name: "TEST",
        lon_deg: 21.01,
        lat_deg: 52.23,
    }];
    let data = build_min_lid(&cities, &stock);
    std::fs::write(&out_path, &data).expect("write");
    println!("wrote {} bytes to {}", data.len(), out_path);
    let nl = lid_format::read(&data).expect("self-read failed");
    verify(&nl.elements);
}
