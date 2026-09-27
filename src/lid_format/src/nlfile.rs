//! THE single raw name-list container builder, shared by the OSM converter (`osm2lid`,
//! `crate::encode_id`) and the example/demo file generation (`crate::city::build_city_file`).
//! One code path = one place where the device's exact header rules (§11.4c of
//! `LID_format.md`) live; every built file is self-checked with the device-faithful
//! `device_sim::check_name_list_header` replica before it is returned.
//!
//! File layout (identical for both callers):
//! `0x77 outer header | u32 elem | 6 x u16 counts | 2 x i32 origin | 7 x {u8 code, u32 off}
//!  | sec0 sec1 sec2 sec3 sec4 sec5 sec6 | blocks... | trailer records (optional)`
//!
//! * `sec0` = VLE block byte-sizes, `sec1` = raw u32 ABSOLUTE block offsets (auto-derived).
//! * `sec2/3` + trailer: optional 35-byte relation records, one per (block, partner); `sec2`
//!   holds VLE record sizes, `sec3` raw u32 absolute trailer offsets (auto-derived).
//! * `sec4/5/6` = opaque id-vector streams supplied verbatim (code + declared count + bytes).

use crate::device_sim::check_name_list_header;
use crate::header::{nl_header, KIND_NAME_LIST, NL_HEADER_LEN};

/// Record stride of the per-block relation records (stock-verified 35 bytes, §11.4c).
pub const REL_RECORD_LEN: usize = 35;

/// One opaque id-vector stream (`sec4`, `sec5` or `sec6`).
#[derive(Debug, Clone)]
pub struct SecStream {
    /// Device codec: `0x11` raw u16, `0x14` VLE, `0x18` Simple9 ...
    pub code: u8,
    /// Declared element count of the vector (device cross-checks it against the decode).
    pub count: u16,
    /// Encoded bytes; must decode to EXACTLY `count` values (device rule).
    pub bytes: Vec<u8>,
}

impl SecStream {
    /// An empty stream (zero count, zero span).
    pub fn empty(code: u8) -> Self {
        SecStream {
            code,
            count: 0,
            bytes: Vec::new(),
        }
    }
}

/// The 35-byte relation records replicated per block (city-list shape; `None` for plain
/// name lists, matching stock street/gazetteer files which carry no records).
#[derive(Debug, Clone)]
pub struct RelRecords {
    /// `partner_elem_count` word of each record (one record per partner per block).
    pub partners: Vec<u32>,
    /// The stock 3-byte record tail (`rec[32..35]`).
    pub tail: [u8; 3],
}

/// Full description of a raw name-list file.
pub struct NameListSpec<'a> {
    /// `rIdxListID.regionIdent` (POL stock: `0x0402`).
    pub region: u16,
    /// `rIdxListID.listID` (city list = 2, street list = 3, ...).
    pub list_id: u16,
    /// Global element count (u32; device `GetAsfSubHeader` base word).
    pub elem_count: u32,
    /// Position quantization shift (stock POL: 8).
    pub shift: u16,
    /// File-level position origin (PAU).
    pub origin: (i32, i32),
    /// Pre-built block images in file order.
    pub blocks: Vec<Vec<u8>>,
    pub sec4: SecStream,
    pub sec5: SecStream,
    pub sec6: SecStream,
    /// Per-block relation records (`None` = street/gazetteer shape, sec2/sec3 empty).
    pub records: Option<RelRecords>,
    /// Block header words read by the record builder: `(node_count, roots)` at block[0..4]
    /// (city-block shape); edge count = node_count - roots. Only consulted when `records`
    /// is `Some`.
    pub block_edges: &'a dyn Fn(&[u8]) -> u32,
}

/// Build the container; the result is verified with the device-faithful LoadHeader replica.
pub fn build_name_list(spec: &NameListSpec<'_>) -> Result<Vec<u8>, String> {
    let nrec = match &spec.records {
        Some(r) => spec.blocks.len() * r.partners.len(),
        None => 0,
    };
    let mut sec0: Vec<u8> = Vec::new();
    for b in &spec.blocks {
        sec0.extend(crate::vle_encode(b.len() as u32));
    }
    let sec1_len = 4 * spec.blocks.len();
    let sec2_len = if nrec > 0 {
        let mut n = 0usize;
        for _ in 0..nrec {
            n += crate::vle_encode(REL_RECORD_LEN as u32).len();
        }
        n
    } else {
        0
    };
    let sec3_len = 4 * nrec;

    // Section table right behind the 24 field bytes of the subheader region.
    let tbl_at = NL_HEADER_LEN + 24;
    let base = tbl_at + 7 * 5;
    let layout = [
        sec0.len(),
        sec1_len,
        sec2_len,
        sec3_len,
        spec.sec4.bytes.len(),
        spec.sec5.bytes.len(),
        spec.sec6.bytes.len(),
    ];
    let mut o = base;
    let mut offs = [0usize; 7];
    for i in 0..7 {
        offs[i] = o;
        o += layout[i];
    }
    let streams_end = o;
    let block_abs = streams_end;
    let blob_abs = block_abs + spec.blocks.iter().map(|b| b.len()).sum::<usize>();
    let trailer_len = nrec * REL_RECORD_LEN;

    let mut f = nl_header(
        KIND_NAME_LIST,
        spec.region,
        spec.list_id,
        (streams_end - NL_HEADER_LEN) as u32,
    );
    f.extend(spec.elem_count.to_le_bytes()); // +0  elem count (u32)
    f.extend((spec.blocks.len() as u16).to_le_bytes()); // +4  nb
    f.extend((nrec as u16).to_le_bytes()); // +6  nrec
    f.extend(spec.sec4.count.to_le_bytes()); // +8  nrel   (sec4 vector size)
    f.extend(spec.sec5.count.to_le_bytes()); // +0x0a
    f.extend(spec.sec6.count.to_le_bytes()); // +0x0c
    f.extend(spec.shift.to_le_bytes()); // +0x0e
    f.extend(spec.origin.0.to_le_bytes()); // +0x10
    f.extend(spec.origin.1.to_le_bytes()); // +0x14
    let codes = [
        0x14u8,
        0x11,
        if nrec > 0 { 0x14 } else { 0x11 },
        0x11,
        spec.sec4.code,
        spec.sec5.code,
        spec.sec6.code,
    ];
    for i in 0..7 {
        f.push(codes[i]);
        f.extend((offs[i] as u32).to_le_bytes());
    }
    debug_assert_eq!(f.len(), base);
    f.resize(streams_end, 0);
    f[offs[0]..][..sec0.len()].copy_from_slice(&sec0);
    let mut boff = block_abs;
    for (i, b) in spec.blocks.iter().enumerate() {
        f[offs[1] + 4 * i..offs[1] + 4 * i + 4].copy_from_slice(&(boff as u32).to_le_bytes());
        boff += b.len();
    }
    if nrec > 0 {
        let mut q = offs[2];
        for _ in 0..nrec {
            let v = crate::vle_encode(REL_RECORD_LEN as u32);
            f[q..q + v.len()].copy_from_slice(&v);
            q += v.len();
        }
        let mut roff = blob_abs;
        for i in 0..nrec {
            f[offs[3] + 4 * i..offs[3] + 4 * i + 4].copy_from_slice(&(roff as u32).to_le_bytes());
            roff += REL_RECORD_LEN;
        }
    }
    for (i, s) in [&spec.sec4, &spec.sec5, &spec.sec6].iter().enumerate() {
        f[offs[4 + i]..][..s.bytes.len()].copy_from_slice(&s.bytes);
    }
    for b in &spec.blocks {
        f.extend_from_slice(b);
    }
    if let Some(rec) = &spec.records {
        for b in &spec.blocks {
            let edges = (spec.block_edges)(b);
            for &p in &rec.partners {
                f.extend((REL_RECORD_LEN as u32).to_le_bytes());
                f.extend(p.to_le_bytes());
                f.extend(edges.to_le_bytes());
                f.extend(
                    [
                        35u32.to_le_bytes(),
                        35u32.to_le_bytes(),
                        35u32.to_le_bytes(),
                    ]
                    .concat(),
                );
                f.extend([0u32.to_le_bytes(), 0u32.to_le_bytes()].concat());
                f.extend_from_slice(&rec.tail);
            }
        }
        debug_assert_eq!(f.len(), blob_abs + trailer_len);
    }

    check_name_list_header(&format!("LID2{:04}", spec.list_id), &f).map_err(|g| g.0)?;
    Ok(f)
}
