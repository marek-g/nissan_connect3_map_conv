// From-scratch CITY name-list (LID2nnnn, listID 2) writer.
//
// Builds a complete, valid file (metadata region + sub-header + 7 streams + trie blocks) from a
// plain list of (name, lon, lat) entries. All device semantics encoded here were RE'd from
// DAPIAPP.OUT: NLNameList::LoadHeader, NLAsfBlock::SetDataBlock/enDecodeTreeStructure,
// ProcessSubTreeTEIC/CalculateTerminatingElementIndex, NLPositionAttrVector::Decode (positions are
// VLE deltas vs the file origin), ReadVle (bijective base-128), and the 36-descriptor template the
// device requires (opts differ per UI path, so every column must decode under any subset).
//
// Element order note: with no name being a prefix of another, TEIC element order == byte order of
// the uppercase UTF-8 names (leaf-children-first adds no reordering without prefix pairs).
use crate::read;
use crate::simple9_encode;
use crate::vle_encode;

/// Position quantization shift written into the city sub-header (u16@+0x0e) and used by the
/// position encoder; stock POL city files carry 8. Device: `abs = origin + (stored << shift)`.
pub const CITY_POS_SHIFT: u16 = 8;

/// Encode one quantized position delta for `CITY_POS_SHIFT` (round-to-nearest).
pub fn qdelta(d: i64) -> i64 {
    let q = 1i64 << (CITY_POS_SHIFT - 1);
    (d + if d >= 0 { q } else { -q }) >> CITY_POS_SHIFT
}

/// One city element: stored name (uppercase UTF-8, as stock carries it) + WGS84 position in PAU
/// (1/11930464.0 deg = 2^31/180).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CityEntry {
    pub name: String,
    pub lon_pau: i32,
    pub lat_pau: i32,
}

impl CityEntry {
    pub fn from_deg(name: &str, lon_deg: f64, lat_deg: f64) -> Self {
        const PAU: f64 = (1i64 << 31) as f64 / 180.0;
        CityEntry {
            name: name.to_string(),
            lon_pau: (lon_deg * PAU) as i32,
            lat_pau: (lat_deg * PAU) as i32,
        }
    }
}

struct TNode {
    children: Vec<(Vec<u8>, usize)>, // (edge label bytes, child idx) — sorted by label
    leaf: bool,
}

fn u16b(v: u16) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}
fn u32b(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

// The 36-row stock descriptor template (row order + codecs byte-identical to stock block45).
// (kind, code, param, payload); stream offsets are computed at emit time (row order spans).
fn template(
    nc: u32,
    ne: u32,
    nbel: u32,
    od_bytes: &[u8],
    blob: &[u8],
    loff_bytes: &[u8],
    claim_bytes: &[u8],
    pos_bytes: &[u8],
) -> Vec<(u16, u16, u32, Vec<u8>)> {
    let s9_zero = simple9_encode(&vec![0u32; nbel as usize], 28);
    vec![
        (0x4402, 0x02, nc, vec![]),
        (0x0402, 0x14, 0, vec![]),
        (0x0401, 0x14, nc, od_bytes.to_vec()),
        (0x4404, 0x02, ne, vec![]),
        (0x8404, 0x11, 0, vec![]),
        (0x0404, 0x11, 0, vec![]),
        (0x4405, 0x02, ne, vec![]),
        (0x8405, 0x11, 0, vec![]),
        (0x0405, 0x11, 0, vec![]),
        (0x8403, 0x14, ne, loff_bytes.to_vec()),
        (0x0403, 0x11, blob.len() as u32, blob.to_vec()),
        (0x0406, 0x14, ne, claim_bytes.to_vec()),
        (0x0413, 0x02, ne, vec![]),
        (0x4407, 0x03, nbel, vec![]),
        (0x0407, 0x14, nbel, pos_bytes.to_vec()), // VLE deltas vs file origin
        (0x440e, 0x02, nbel, vec![]),
        (0x040e, 0x11, 0, vec![]),
        (0x4408, 0x02, nbel, vec![]),
        (0x0408, 0x11, 0, vec![]),
        (0x4409, 0x02, nbel, vec![]),
        (0x8409, 0x11, 0, vec![]),
        (0x0409, 0x11, 0, vec![]),
        (0x440a, 0x02, nbel, vec![]),
        (0x040a, 0x11, 0, vec![]),
        (0x440b, 0x02, nbel, vec![]),
        (0x040b, 0x11, 0, vec![]),
        (0x440c, 0x02, nbel, vec![]),
        (0x040c, 0x11, 0, vec![]),
        (0x8415, 0x18, nbel, s9_zero), // char-status values: non-empty is a HARD gate
        (0x0415, 0x01, 0, vec![]),
        (0x040f, 0x02, nbel, vec![]),
        (0x0410, 0x02, nbel, vec![]),
        (0x0411, 0x02, nbel, vec![]),
        (0x0414, 0x02, nbel, vec![]),
        (0x0412, 0x02, nbel, vec![]),
        (0x040d, 0x03, nbel, vec![]), // valid-destination all-set
    ]
}

/// Build ONE city-list block (trie + 36-row template). Positions are deltas vs `origin`.
/// Returns (block bytes, nbel). Requires: uppercase names, none a prefix of another.
pub fn build_city_block(cities: &[CityEntry], origin: (i32, i32)) -> (Vec<u8>, usize) {
    assert!(!cities.is_empty(), "city list must be non-empty");
    let mut sorted: Vec<&CityEntry> = cities.iter().collect();
    sorted.sort_unstable_by_key(|c| c.name.as_bytes());
    for w in sorted.windows(2) {
        assert!(
            !w[1].name.as_bytes().starts_with(w[0].name.as_bytes()),
            "prefix city names unsupported: {} / {}",
            w[0].name,
            w[1].name
        );
    }

    // ---- trie (shared prefix), children kept in label-byte order ----
    let mut nodes: Vec<TNode> = vec![TNode {
        children: Vec::new(),
        leaf: false,
    }];
    for c in sorted.iter() {
        let mut cur = 0usize;
        for ch in c.name.chars() {
            let mut tmp = [0u8; 4];
            let lab = ch.encode_utf8(&mut tmp).as_bytes().to_vec();
            cur = match nodes[cur].children.iter().position(|(l, _)| *l == lab) {
                Some(i) => nodes[cur].children[i].1,
                None => {
                    nodes.push(TNode {
                        children: Vec::new(),
                        leaf: false,
                    });
                    let id = nodes.len() - 1;
                    let cs = &mut nodes[cur].children;
                    cs.push((lab, id));
                    cs.sort_by(|a, b| a.0.cmp(&b.0));
                    id
                }
            };
        }
        nodes[cur].leaf = true;
    }

    // ---- device node-id assignment: child i of n = treeBase + fe[n] + i ----
    // Simulate the numbering loop (stack DFS, `root += visits`) and re-index the trie so
    // nodes[id] matches what the device/reader derives from outDegree alone.
    let nct = nodes.len();
    let mut ids: Vec<usize> = vec![usize::MAX; nct];
    {
        let mut root = 0usize;
        let mut base = 1usize;
        let mut ec = 0usize;
        let mut stack: Vec<(usize, usize)> = vec![(0usize, 0usize)];
        while root < nct {
            let mut visits = 0usize;
            while let Some((t, id)) = stack.pop() {
                ids[t] = id;
                let cs_n = base + ec;
                ec += nodes[t].children.len();
                visits += 1;
                for (i, (_, c)) in nodes[t].children.iter().enumerate().rev() {
                    if cs_n + i < nct {
                        stack.push((*c, cs_n + i));
                    }
                }
            }
            root += visits;
            base += 1;
            if root < nct {
                stack.push((root, root));
            }
        }
    }
    assert!(!ids.iter().any(|&x| x == usize::MAX), "unnumbered node");
    let mut po: Vec<TNode> = (0..nct)
        .map(|_| TNode {
            children: Vec::new(),
            leaf: false,
        })
        .collect();
    for t in 0..nct {
        po[ids[t]] = TNode {
            children: nodes[t]
                .children
                .iter()
                .map(|(l, c)| (l.clone(), ids[*c]))
                .collect(),
            leaf: nodes[t].leaf,
        };
    }
    let nodes = po;

    // ---- streams over the numbered tree (same DFS discipline as the device loop) ----
    let nc = nct;
    let mut od = vec![0u32; nc];
    let mut claims: Vec<u32> = Vec::new();
    let mut blob: Vec<u8> = Vec::new();
    let mut loffs: Vec<u32> = Vec::new();
    fn sub_leaves(nodes: &[TNode], n: usize) -> usize {
        let mut c = if nodes[n].leaf { 1 } else { 0 };
        for (_, ch) in &nodes[n].children {
            c += sub_leaves(nodes, *ch);
        }
        c
    }
    {
        let mut root = 0usize;
        let mut base = 1usize;
        let mut ec = 0usize;
        let mut stack = vec![0usize];
        while root < nc {
            let mut visits = 0usize;
            while let Some(n) = stack.pop() {
                let cs_n = base + ec;
                od[n] = nodes[n].children.len() as u32;
                for (lab, ch) in nodes[n].children.iter() {
                    loffs.push(blob.len() as u32);
                    blob.extend_from_slice(lab);
                    claims.push(sub_leaves(&nodes, *ch) as u32);
                }
                ec += nodes[n].children.len();
                visits += 1;
                for i in (0..nodes[n].children.len()).rev() {
                    if cs_n + i < nc {
                        stack.push(cs_n + i);
                    }
                }
            }
            root += visits;
            base += 1;
        }
    }
    let ne = claims.len();
    let nbel = sorted.len();

    // ---- positions: VLE quantized deltas vs file origin (stock city-file shape) ----
    // Device model (NLPositionAttrVector, CONFIRMED 2026-09-25): abs = origin + (stored << shift)
    // with shift/origin as file-global constants (sub-header u16@+0x0e, i32@+0x10/+0x14).
    // Stock POL city file uses shift=8, so stored = round((abs - origin) / 2^shift).
    let mut pos_bytes = Vec::new();
    for c in sorted.iter() {
        pos_bytes.extend_from_slice(&vle_encode(qdelta(c.lon_pau as i64 - origin.0 as i64) as u32));
        pos_bytes.extend_from_slice(&vle_encode(qdelta(c.lat_pau as i64 - origin.1 as i64) as u32));
    }
    let od_bytes: Vec<u8> = od.iter().flat_map(|&d| vle_encode(d)).collect();
    let claim_bytes: Vec<u8> = claims.iter().flat_map(|&c| vle_encode(c)).collect();
    let loff_bytes: Vec<u8> = loffs.iter().flat_map(|&o| vle_encode(o)).collect();

    let rws = template(
        nc as u32,
        ne as u32,
        nbel as u32,
        &od_bytes,
        &blob,
        &loff_bytes,
        &claim_bytes,
        &pos_bytes,
    );
    let data_base = 8 + 12 * rws.len();
    let mut data = Vec::new();
    let mut offs = Vec::new();
    for (_, _, _, p) in rws.iter() {
        offs.push((data_base + data.len()) as u32);
        data.extend_from_slice(p);
    }
    let mut block = Vec::new();
    block.extend_from_slice(&u16b(nc as u16));
    let sum_od: usize = od.iter().map(|&d| d as usize).sum();
    block.extend_from_slice(&u16b((nc - sum_od) as u16)); // forest roots
    block.extend_from_slice(&u16b(nbel as u16));
    block.extend_from_slice(&u16b(rws.len() as u16));
    for i in 0..rws.len() {
        let (k, c, p, _) = rws[i];
        block.extend_from_slice(&u16b(k));
        block.extend_from_slice(&u16b(c));
        block.extend_from_slice(&u32b(offs[i]));
        block.extend_from_slice(&u32b(p));
    }
    block.extend_from_slice(&data);
    (block, nbel)
}

/// Metadata/constant region copied verbatim from a stock city file (region1 strings, file-spec
/// constants, sec4/5/6 tables). `stock` must be the stock POL LID20001.DAT raw bytes.
/// Returns (file bytes). Single block; origin = stock sub-header constants (device adds them to
/// the stored position deltas).
pub fn build_city_file(cities: &[CityEntry], stock: &[u8]) -> Vec<u8> {
    let hdr = 119usize;
    let origin = (
        i32::from_le_bytes(stock[hdr + 16..hdr + 20].try_into().unwrap()),
        i32::from_le_bytes(stock[hdr + 20..hdr + 24].try_into().unwrap()),
    );
    let (block, _nbel) = build_city_block(cities, origin);
    build_city_container(cities.len() as u32, &[block], stock, hdr)
}

/// Regenerated `REL00000/1/3/6` matrices for a from-scratch city list (all four target the new
/// file's list-2 element ids `0..cities.len()`). The device derives city candidates from these
/// matrices (`vPopulateCityIndices` reads `LISA_tclRelationMap` ranges), so shipping a new
/// `LID20001` without them leaves the city browser structurally empty (card-verified 2026-09-25).
#[derive(Debug)]
pub struct CityRelations {
    pub rel0: Vec<u8>,
    pub rel1: Vec<u8>,
    pub rel3: Vec<u8>,
    pub rel6: Vec<u8>,
}

/// Map a stock name-list element position+name to one of `cities` (element index), requiring an
/// ASCII-folded name match AND a <= 15 km distance (kills homonym villages).
fn city_match(cities: &[CityEntry], ax: i32, ay: i32, name: &str) -> Option<usize> {
    let deg = 2f64.powi(31) / 180.0;
    let lon = f64::from(ax) / deg;
    let lat = f64::from(ay) / deg;
    let folded = fold_polish(&name.to_uppercase());
    let mut best: Option<(f64, usize)> = None;
    for (i, c) in cities.iter().enumerate() {
        if fold_polish(&c.name.to_uppercase()) != folded {
            continue;
        }
        let (clon, clat) = (c.lon_pau as f64 / deg, c.lat_pau as f64 / deg);
        let dx = (lon - clon) * lat.to_radians().cos();
        let dy = lat - clat;
        let km = (dx * dx + dy * dy).sqrt() * 111.194_926_644_558_73;
        if best.is_none_or(|(b, _)| km < b) {
            best = Some((km, i));
        }
    }
    best.filter(|(km, _)| *km <= 15.0).map(|(_, i)| i)
}

/// ASCII-fold the Polish diacritic set (uppercase input).
fn fold_polish(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'Ą' => 'A',
            'Ć' => 'C',
            'Ę' => 'E',
            'Ł' => 'L',
            'Ń' => 'N',
            'Ó' => 'O',
            'Ś' => 'S',
            'Ź' => 'Z',
            'Ż' => 'Z',
            other => other,
        })
        .collect()
}

/// Full-scan every pair of a stock REL file (column query over the whole source range).
fn all_pairs(b: &[u8]) -> Result<Vec<(u32, u32)>, String> {
    let idx = crate::rel::RelIndex::parse(b)?;
    crate::rel::get_relations(b, &idx, true, 0, idx.d[2])
}

/// Build the four device REL matrices (`REL00000` self, `REL00001` street->city, `REL00003`
/// district->city, `REL00006` region->city) consistent with a new city file of `cities.len()`
/// elements. Stock matrices are re-mapped: a stock endpoint that matches one of `cities`
/// (ASCII-folded name within 15 km) is replaced by that city's new element id; `REL00001`
/// targets (street owners) additionally fall back to the geographically nearest new city so
/// every street keeps an owner. Stock source-side ids (districts/regions/streets) are kept.
pub fn build_city_relations(
    stock_lid: &[u8],
    stock_rel0: &[u8],
    stock_rel1: &[u8],
    stock_rel3: &[u8],
    stock_rel6: &[u8],
    cities: &[CityEntry],
) -> Result<CityRelations, String> {
    use crate::rel::write_rel;
    let nl = read(stock_lid).map_err(|e| format!("city-rel: stock lid: {e}"))?;
    let (ox, oy) = nl.origin.ok_or("city-rel: stock lid has no origin")?;
    let sh = crate::pos_shift(stock_lid) as u32;
    let n_new = cities.len() as u64;
    let deg = 2f64.powi(31) / 180.0;
    // nearest city (unbounded) per stock element, for REL00001 target owners.
    let nearest_city = |ax: i64, ay: i64| -> u32 {
        let lon = ax as f64 / deg;
        let lat = ay as f64 / deg;
        let mut best = (0usize, f64::MAX);
        for (i, c) in cities.iter().enumerate() {
            let dx = (lon - c.lon_pau as f64 / deg) * lat.to_radians().cos();
            let dy = lat - c.lat_pau as f64 / deg;
            let d = dx * dx + dy * dy;
            if d < best.1 {
                best = (i, d);
            }
        }
        best.0 as u32
    };
    let abs = |e: &crate::Element| (ox as i64 + ((e.x_pau as i64) << sh), oy as i64 + ((e.y_pau as i64) << sh));
    let mapped: Vec<Option<usize>> = nl
        .elements
        .iter()
        .map(|e| {
            let (ax, ay) = abs(e);
            city_match(cities, ax as i32, ay as i32, &e.name)
        })
        .collect();
    let tgt = |id: u32, fallback: bool| -> Option<u32> {
        match mapped.get(id as usize).copied().flatten() {
            Some(i) => Some(i as u32),
            None if fallback => {
                let e = &nl.elements[id as usize];
                let (ax, ay) = abs(e);
                Some(nearest_city(ax, ay))
            }
            None => None,
        }
    };
    let dedup = |v: &mut Vec<(u32, u32)>| {
        v.sort_unstable();
        v.dedup();
    };
    // REL00000 (city<->city urban parts): keep stock pairs whose both ends belong to the same
    // new city, and guarantee a self-pair per city (stock links cities to their urban-part
    // elements, whose names differ; a new list has none, so only (i,i) is meaningful — the FLI
    // row query then simply returns the matched city itself).
    let mut p0: Vec<(u32, u32)> = (0..cities.len() as u32).map(|i| (i, i)).collect();
    for (s, t) in all_pairs(stock_rel0)? {
        if let (Some(i), Some(j)) = (tgt(s, false), tgt(t, false)) {
            if i == j {
                p0.push((i, j));
            }
        }
    }
    dedup(&mut p0);
    // REL00001 (street->city): re-own every stock street to its (fallback nearest) new city.
    let r1 = crate::rel::RelIndex::parse(stock_rel1)?;
    let mut p1: Vec<(u32, u32)> = all_pairs(stock_rel1)?
        .into_iter()
        .filter_map(|(s, t)| tgt(t, true).map(|k| (s, k)))
        .collect();
    dedup(&mut p1);
    // REL00003 (district->city) / REL00006 (region->city): keep matched cities.
    let remap_side = |stock: &[u8]| -> Result<Vec<(u32, u32)>, String> {
        let mut v: Vec<(u32, u32)> = all_pairs(stock)?
            .into_iter()
            .filter_map(|(s, t)| tgt(t, false).map(|k| (s, k)))
            .collect();
        dedup(&mut v);
        Ok(v)
    };
    let p3 = remap_side(stock_rel3)?;
    let p6 = remap_side(stock_rel6)?;
    let n1 = u64::from(r1.d[2]);
    let n3 = u64::from(crate::rel::RelIndex::parse(stock_rel3)?.d[2]);
    let n6 = u64::from(crate::rel::RelIndex::parse(stock_rel6)?.d[2]);
    Ok(CityRelations {
        rel0: write_rel(n_new, n_new, 2, 2, &p0)?,
        rel1: write_rel(n1, n_new, 3, 2, &p1)?,
        rel3: write_rel(n3, n_new, 10, 2, &p3)?,
        rel6: write_rel(n6, n_new, 9, 2, &p6)?,
    })
}

// two-pass container: region1 + sub-header + section table + streams + blocks + record blob
fn build_city_container(
    elem_count: u32,
    blocks: &[Vec<u8>],
    stock: &[u8],
    hdr: usize,
) -> Vec<u8> {
    let nb = blocks.len();
    let sec4 = stock[1393..1397].to_vec();
    let sec5 = stock[1397..1400].to_vec();
    let sec6 = stock[1400..1428].to_vec();
    let sec0: Vec<u8> = blocks.iter().flat_map(|b| vle_encode(b.len() as u32)).collect();
    let sec1_len = 4 * nb;

    // relation records (sec2/sec3): the stock name-list header promises nrel relations; each
    // block carries one 35-byte record per relation: [u32 size][u32 partner elem count]
    // [u32 block edge count][u32 35 x3][u32 0 x2][3-byte tail]. Stock device code iterates
    // nrel per block; a header promising relations with zero records was the top suspect for
    // the 2026-09-25 10-city card reset. Partner counts + tail are lifted from the stock
    // file's own block-0 records so the list stays consistent with the stock REL*.DAT files.
    let nrec_stock = u16::from_le_bytes(stock[hdr + 6..hdr + 8].try_into().unwrap()) as usize;
    let nrel = u16::from_le_bytes(stock[hdr + 8..hdr + 10].try_into().unwrap()) as usize;
    assert_eq!(nrec_stock, 45 * nrel, "unexpected stock record count");
    let sec3_off = u32::from_le_bytes(stock[hdr + 24 + 3 * 5 + 1..hdr + 24 + 3 * 5 + 5].try_into().unwrap()) as usize;
    let rec0 = u32::from_le_bytes(stock[sec3_off..sec3_off + 4].try_into().unwrap()) as usize;
    let rec_stride = u32::from_le_bytes(stock[sec3_off + 4..sec3_off + 8].try_into().unwrap()) as usize - rec0;
    assert_eq!(rec_stride, 35, "unexpected stock record size");
    let mut partners = Vec::with_capacity(nrel);
    let stock_elems = u32::from_le_bytes(stock[hdr..hdr + 4].try_into().unwrap());
    for r in 0..nrel {
        let f1 = u32::from_le_bytes(
            stock[rec0 + r * rec_stride + 4..rec0 + r * rec_stride + 8]
                .try_into()
                .unwrap(),
        );
        // the self-relation record names OUR own element count as partner, not the stock's.
        partners.push(if f1 == stock_elems { elem_count } else { f1 });
    }
    let rec_tail = stock[rec0 + 32..rec0 + 35].to_vec();
    let nrec = nrel * nb;
    let mut sec2: Vec<u8> = Vec::new();
    let mut rec_blob: Vec<u8> = Vec::new();
    for b in blocks.iter() {
        let nc = u16::from_le_bytes(b[0..2].try_into().unwrap()) as usize;
        let roots = u16::from_le_bytes(b[2..4].try_into().unwrap()) as usize;
        let sum_od = (nc - roots) as u32;
        for &p in partners.iter() {
            sec2.extend_from_slice(&vle_encode(35));
            rec_blob.extend_from_slice(&u32b(35));
            rec_blob.extend_from_slice(&u32b(p));
            rec_blob.extend_from_slice(&u32b(sum_od));
            rec_blob.extend_from_slice(&u32b(35));
            rec_blob.extend_from_slice(&u32b(35));
            rec_blob.extend_from_slice(&u32b(35));
            rec_blob.extend_from_slice(&u32b(0));
            rec_blob.extend_from_slice(&u32b(0));
            rec_blob.extend_from_slice(&rec_tail);
        }
    }
    let sec3_len = 4 * nrec;

    let tbl_at = hdr + 24;
    let base = tbl_at + 35;
    let layout = [
        sec0.len(),
        sec1_len,
        sec2.len(),
        sec3_len,
        sec4.len(),
        sec5.len(),
        sec6.len(),
    ];
    let mut o = base;
    let offs_sec: Vec<usize> = layout.iter().map(|l| {
        let c = o;
        o += l;
        c
    }).collect();
    let streams_end = o;
    let block_abs = streams_end;
    let blob_abs = block_abs + blocks.iter().map(|b| b.len()).sum::<usize>();

    let mut s: Vec<u8> =
        Vec::with_capacity(blob_abs + rec_blob.len());
    s.extend_from_slice(&u16b(0x0402));
    s.extend_from_slice(&u16b(2)); // listID = 2 (city list; drives the UI category filter)
    s.extend_from_slice(&u16b(0));
    s.extend_from_slice(&u16b(0x41ec));
    s.extend_from_slice(&u32b(0));
    s.extend_from_slice(&u32b(1));
    s.extend_from_slice(&u32b(hdr as u32));
    s.extend_from_slice(&u32b(0)); // header size: patched below
    s.extend_from_slice(&u16b(38));
    s.extend_from_slice(&u16b(84));
    s.extend_from_slice(&u16b(95));
    s.extend_from_slice(&u16b(96));
    s.extend_from_slice(&u16b(13));
    s.extend_from_slice(&u16b(2));
    s.extend_from_slice(&u16b(118));
    s.extend_from_slice(&stock[38..hdr]);
    assert_eq!(s.len(), hdr);
    s.extend_from_slice(&u32b(elem_count));
    s.extend_from_slice(&u16b(nb as u16));
    s.extend_from_slice(&u16b(nrec as u16)); // nrec: one record per (block, relation)
    s.extend_from_slice(&u16b(4));
    s.extend_from_slice(&u16b(3));
    s.extend_from_slice(&u16b(29));
    s.extend_from_slice(&u16b(CITY_POS_SHIFT));
    s.extend_from_slice(&stock[hdr + 16..hdr + 24]); // file-spec constants / position origin
    assert_eq!(s.len(), tbl_at);
    let codes: [u8; 7] = [0x14, 0x11, 0x14, 0x11, 0x14, 0x14, 0x18];
    for (i, c) in codes.iter().enumerate() {
        s.push(*c);
        s.extend_from_slice(&u32b(offs_sec[i] as u32));
    }
    assert_eq!(s.len(), base);
    s.resize(streams_end, 0);
    s[offs_sec[0]..][..sec0.len()].copy_from_slice(&sec0);
    s[offs_sec[2]..][..sec2.len()].copy_from_slice(&sec2);
    let mut roff = blob_abs;
    for i in 0..nrec {
        s[offs_sec[3] + 4 * i..offs_sec[3] + 4 * i + 4].copy_from_slice(&u32b(roff as u32));
        roff += 35;
    }
    let mut boff = block_abs;
    for (i, blk) in blocks.iter().enumerate() {
        s[offs_sec[1] + 4 * i..offs_sec[1] + 4 * i + 4].copy_from_slice(&u32b(boff as u32));
        boff += blk.len();
    }
    s[offs_sec[4]..][..sec4.len()].copy_from_slice(&sec4);
    s[offs_sec[5]..][..sec5.len()].copy_from_slice(&sec5);
    s[offs_sec[6]..][..sec6.len()].copy_from_slice(&sec6);
    for blk in blocks.iter() {
        s.extend_from_slice(blk);
    }
    s.extend_from_slice(&rec_blob);
    assert_eq!(s.len(), blob_abs + rec_blob.len());
    s[20..24].copy_from_slice(&u32b((streams_end - hdr) as u32));
    s
}

/// Self-check via the crate reader: names, order and absolute positions must round-trip.
pub fn selfcheck(data: &[u8], stock: &[u8], cities: &[CityEntry]) -> Result<(), String> {
    let nl = read(data)?;
    let mut want: Vec<&CityEntry> = cities.iter().collect();
    want.sort_unstable_by_key(|c| c.name.as_bytes());
    if nl.elements.len() != want.len() {
        return Err(format!(
            "element count {} != {}",
            nl.elements.len(),
            want.len()
        ));
    }
    let ox = i32::from_le_bytes(stock[119 + 16..119 + 20].try_into().unwrap()) as i64;
    let oy = i32::from_le_bytes(stock[119 + 20..119 + 24].try_into().unwrap()) as i64;
    let q = 1i64 << CITY_POS_SHIFT;
    for (e, w) in nl.elements.iter().zip(want.iter()) {
        if e.name != w.name {
            return Err(format!("name {} != expected {}", e.name, w.name));
        }
        let ax = ox + ((e.x_pau as i64) << CITY_POS_SHIFT);
        let ay = oy + ((e.y_pau as i64) << CITY_POS_SHIFT);
        if (ax - w.lon_pau as i64).abs() > q || (ay - w.lat_pau as i64).abs() > q {
            return Err(format!(
                "position mismatch for {}: ({ax},{ay}) != ({},{})",
                w.name, w.lon_pau, w.lat_pau
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock() -> Vec<u8> {
        std::fs::read(
            "/home/marek/Ext/reverse_engineering/NissanMaps/Firmware/Map_unpacked/\
             CRYPTNAV/DATA/DATA/LID/CCP/POL/LID20001.DAT",
        )
        .expect("stock LID20001 present")
    }

    fn ten() -> Vec<CityEntry> {
        [
            ("BYDGOSZCZ", 18.0084, 53.1235),
            ("GDAŃSK", 18.6464, 54.3520),
            ("KRAKÓW", 19.9450, 50.0647),
            ("LUBLIN", 22.5684, 51.2465),
            ("POZNAŃ", 16.9252, 52.4064),
            ("RADOM", 21.1471, 51.4027),
            ("SZCZECIN", 14.5528, 53.4285),
            ("WARSZAWA", 21.0118, 52.2298),
            ("WROCŁAW", 17.0385, 51.1079),
            ("ŁÓDŹ", 19.4560, 51.7592),
        ]
        .iter()
        .map(|(n, lo, la)| CityEntry::from_deg(n, *lo, *la))
        .collect()
    }

    #[test]
    fn ten_city_round_trip() {
        let s = stock();
        let data = build_city_file(&ten(), &s);
        selfcheck(&data, &s, &ten()).expect("selfcheck");
    }

    #[test]
    fn single_city_round_trip() {
        let s = stock();
        let one = vec![CityEntry::from_deg("TEST", 21.01, 52.23)];
        let data = build_city_file(&one, &s);
        selfcheck(&data, &s, &one).expect("selfcheck");
    }
}
