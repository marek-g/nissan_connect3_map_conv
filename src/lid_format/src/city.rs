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

    /// Return a copy whose stored name carries an ASCII-folded **search key** as the first
    /// TAB-separated line (line 1 = sort/type-in key, line 2 = diacritic display form), exactly
    /// as stock carries diacritic city names (`GDANSK\tGDAŃSK`, `KRAKOW\tKRAKÓW`). The device
    /// searches line 1 (ASCII-folded type-in), so this makes a diacritic city match an ASCII
    /// query (`GDANSK`) while still displaying the diacritic form. Non-diacritic names are
    /// returned unchanged. Element count is preserved (one element per logical city).
    pub fn with_ascii_fold(&self) -> CityEntry {
        let folded = fold_polish(&self.name);
        if folded == self.name {
            self.clone()
        } else {
            CityEntry {
                name: format!("{folded}\t{}", self.name),
                lon_pau: self.lon_pau,
                lat_pau: self.lat_pau,
            }
        }
    }

    /// The ASCII-folded search/sort key = the first TAB-separated line of the stored name.
    pub fn search_key(&self) -> String {
        fold_polish(self.name.split('\t').next().unwrap_or(&self.name))
    }

    /// The display form = everything after the first TAB (or the whole name when there is none).
    pub fn display_name(&self) -> &str {
        self.name.split('\t').nth(1).unwrap_or(&self.name)
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

// The 36-row stock descriptor template + one safety row (see dummy note in `template`).
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
        // Dummy 37th row: the LAST TOC row's window = u32 read past the TOC (device reads it as
        // start[last] + window from file bytes past the block buffer = heap). With count 0 the
        // decode loop never runs, making the garbage window harmless, and 0x40d above gets
        // window = start(dummy) - start(0x40d) = 0 -> code 3 decodes to all-TRUE without any
        // stream read. (Stock puts 0x40d last and eats a heap read: 8 random clears per 5000,
        // invisible at scale, fatal for a 10-city list - card-verified empty list 2026-09-25.)
        (0x0416, 0x02, 0, vec![]),
    ]
}

/// Build ONE city-list block (trie + 37-row template). Positions are deltas vs `origin`.
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
        pos_bytes.extend_from_slice(&vle_encode(
            qdelta(c.lon_pau as i64 - origin.0 as i64) as u32
        ));
        pos_bytes.extend_from_slice(&vle_encode(
            qdelta(c.lat_pau as i64 - origin.1 as i64) as u32
        ));
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
        if c.search_key() != folded {
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
    let abs = |e: &crate::Element| {
        (
            ox as i64 + ((e.x_pau as i64) << sh),
            oy as i64 + ((e.y_pau as i64) << sh),
        )
    };
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
/// City-list container = shared `crate::nlfile` builder + the stock city-list extras:
/// sec4/5/6 streams (REL ids, file categories, 29 language ids) copied verbatim from the
/// stock POL file, and the 35-byte per-(block,relation) records whose partner counts and
/// tail come from stock block 0 (the self-relation partner is re-pointed to `elem_count`).
fn build_city_container(elem_count: u32, blocks: &[Vec<u8>], stock: &[u8], hdr: usize) -> Vec<u8> {
    // stock sec4/5/6: raw stream bytes + declared counts + codes (subheader table spans).
    // sec6's span ends at hdr+reg (reg = stream region size, main header u32 @0x14).
    let blob_end = hdr + u32::from_le_bytes(stock[0x14..0x18].try_into().unwrap()) as usize;
    let tbl = hdr + 24;
    let sec_off = |i: usize| {
        u32::from_le_bytes(stock[tbl + i * 5 + 1..tbl + i * 5 + 5].try_into().unwrap()) as usize
    };
    let sec_code = |i: usize| stock[tbl + i * 5];
    let ends = [sec_off(4), sec_off(5), sec_off(6), blob_end];
    let cnt = |o: usize| u16::from_le_bytes(stock[hdr + o..hdr + o + 2].try_into().unwrap());
    let sec_stream = |i: usize| crate::nlfile::SecStream {
        code: sec_code(i + 4),
        count: cnt(8 + i * 2),
        bytes: stock[ends[i]..ends[i + 1]].to_vec(),
    };

    // relation records: the stock name-list header promises nrel relations; each block carries
    // one 35-byte record per relation: [u32 size][u32 partner elem count][u32 edge count]
    // [u32 35 x3][u32 0 x2][3-byte tail]. Partner counts + tail are lifted from the stock
    // file's own block-0 records so the list stays consistent with the stock REL*.DAT files.
    let nrec_stock = u16::from_le_bytes(stock[hdr + 6..hdr + 8].try_into().unwrap()) as usize;
    let nrel = u16::from_le_bytes(stock[hdr + 8..hdr + 10].try_into().unwrap()) as usize;
    assert_eq!(nrec_stock, 45 * nrel, "unexpected stock record count");
    let sec3_off = sec_off(3);
    let rec0 = u32::from_le_bytes(stock[sec3_off..sec3_off + 4].try_into().unwrap()) as usize;
    let rec_stride =
        u32::from_le_bytes(stock[sec3_off + 4..sec3_off + 8].try_into().unwrap()) as usize - rec0;
    assert_eq!(
        rec_stride,
        crate::nlfile::REL_RECORD_LEN,
        "unexpected stock record size"
    );
    let stock_elems = u32::from_le_bytes(stock[hdr..hdr + 4].try_into().unwrap());
    let mut partners = Vec::with_capacity(nrel);
    for r in 0..nrel {
        let f1 = u32::from_le_bytes(
            stock[rec0 + r * rec_stride + 4..rec0 + r * rec_stride + 8]
                .try_into()
                .unwrap(),
        );
        // the self-relation record names OUR own element count as partner, not the stock's.
        partners.push(if f1 == stock_elems { elem_count } else { f1 });
    }
    let mut tail = [0u8; 3];
    tail.copy_from_slice(&stock[rec0 + 32..rec0 + 35]);

    let spec = crate::nlfile::NameListSpec {
        region: 0x0402,
        list_id: 2, // city list; drives the UI category filter
        elem_count,
        shift: CITY_POS_SHIFT,
        origin: (
            i32::from_le_bytes(stock[hdr + 16..hdr + 20].try_into().unwrap()),
            i32::from_le_bytes(stock[hdr + 20..hdr + 24].try_into().unwrap()),
        ),
        blocks: blocks.to_vec(),
        sec4: sec_stream(0),
        sec5: sec_stream(1),
        sec6: sec_stream(2),
        records: Some(crate::nlfile::RelRecords { partners, tail }),
        // city block header words: node count @0, roots @2 (device block template §12).
        block_edges: &|b: &[u8]| {
            let nc = u16::from_le_bytes(b[0..2].try_into().unwrap()) as u32;
            let roots = u16::from_le_bytes(b[2..4].try_into().unwrap()) as u32;
            nc - roots
        },
    };
    match crate::nlfile::build_name_list(&spec) {
        Ok(f) => f,
        Err(e) => panic!("build_city_file produced an un-loadable file: {e}"),
    }
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
        // `e.name` is the display form (post-first-TAB); compare against the entry's display line.
        if e.name != w.display_name() {
            return Err(format!("name {} != expected {}", e.name, w.display_name()));
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

    #[test]
    fn block_load_no_stream_reads_past_block() {
        // Device model (NLAsfBlock::SetDataBlock): last-TOC-row window is file garbage read past
        // the block buffer; on the card it hits heap -> random validDestination clears -> empty
        // city list (card-verified 2026-09-25). Generated blocks must never read past the buffer.
        use crate::device_sim::{check_block_load, check_name_list_header};
        let s = stock();
        for cities in [vec![CityEntry::from_deg("TEST", 21.01, 52.23)], ten()] {
            let data = build_city_file(&cities, &s);
            let h = check_name_list_header("zz.DAT", &data).expect("LoadHeader");
            let bl = check_block_load("zz.DAT", &data, &h, 0).expect("SetDataBlock");
            assert!(bl.danger.is_empty(), "overruns: {:?}", bl.danger);
            assert_eq!(
                bl.valid_dest_popcount as u32,
                u32::from(bl.ne),
                "validDestination"
            );
            assert_eq!(bl.root_edges.len(), if cities.len() == 1 { 1 } else { 9 });
        }
    }

    #[test]
    fn element_domain_column_validator() {
        // Faithful column-values validator (vPopulateCityIndices 00c73b68 gate chain): a generated city
        // file must PASS it (char-status present, no plain city permutation-dropped), and the validator
        // must CATCH a file whose char-status column is empty (enGetEntryCharacterStatus returns 4 for
        // every element -> enGetAllElementProperties fails -> all candidates drop).
        use crate::device_sim::{block_toc, check_block_load, check_name_list_header};
        let s = stock();
        let data = build_city_file(&ten(), &s);
        let h = check_name_list_header("zz.DAT", &data).expect("LoadHeader");

        // (1) normal file: char-status covers every element; no element marked permutated.
        let bl = check_block_load("zz.DAT", &data, &h, 0).expect("SetDataBlock");
        assert_eq!(
            bl.char_status_count as u32,
            u32::from(bl.ne),
            "char-status (0x8415) must cover all {ne} elements",
            ne = bl.ne
        );
        assert!(
            bl.perm_dropped.iter().all(|&p| !p),
            "plain cities must not be permutation-dropped: {:?}",
            bl.perm_dropped
        );
        assert!(
            !bl.danger.iter().any(|d| d.contains("char-status count 0")),
            "no false char-status danger on a valid file: {:?}",
            bl.danger
        );

        // (2) zero the 0x8415 descriptor count -> the device's enGetEntryCharacterStatus hard-fails.
        let mut bad = data.clone();
        let rows = block_toc(&bad, &h, 0);
        let i = rows
            .iter()
            .position(|(tag, _, _, _)| *tag == 0x8415)
            .expect("0x8415 descriptor present");
        let cnt_off = h.blob_end + 8 + i * 12 + 8; // TOC entry: tag(2) code(2) start(4) count(4)
        bad[cnt_off..cnt_off + 4].copy_from_slice(&0u32.to_le_bytes());
        let bl_bad = check_block_load("zz.DAT", &bad, &h, 0).expect("SetDataBlock (count 0 still loads)");
        assert_eq!(bl_bad.char_status_count, 0, "mutated char-status count must read back as 0");
        assert!(
            bl_bad.danger.iter().any(|d| d.contains("char-status count 0")),
            "validator must flag the empty char-status column: {:?}",
            bl_bad.danger
        );
    }
}
