//! Bosch TravelMap (Nissan LCN2KAI) CPRNAV_2 codec — universal library.
//!
//! One crate, both directions: [`compress`] packs a buffer into a `CPRNAV_2`
//! archive and [`decompress`] expands one back. The compressor is the exact
//! inverse of the decompressor (a Rust port of the firmware
//! `cpr_tclDecompressAlgorithm` / `cpr_tclFileheader`); files produced by
//! [`compress`] round-trip byte-exactly through [`decompress`] and through the
//! firmware.
//!
//! The two command-line front-ends (`cprnav_compress_rs`, `cprnav_decompress_rs`)
//! are thin wrappers over this crate; everything the format needs lives here so
//! the tables, bit codecs and LZ77 are defined once.
//!
//! # File layout (little-endian)
//! ```text
//! [0x00] u16 version = 5
//! [0x02] u16 block_size_kib  (block_size = block_size_kib * 0x400 bytes; 16 -> 0x4000, 64 -> 0x10000)
//! [0x04] "CPRNAV_2"
//! [0x0c] u32 unpacked_size
//! [0x10] u8  mode = 3
//! [0x11] u8  0
//! [0x12] u16 1
//! [0x14] u32 first = 0x18 + 4*nblocks   (offset of block data)
//! [0x18 + 4*i] u32 end offset of block i   (i in 0..nblocks)
//! [first .. ] block data
//! ```
//!
//! Every block's stored length is a MULTIPLE OF 4 bytes (zero-padded, see `pad4`).
//! Bosch does the same: DAPIAPP.OUT `bDecompressData` guards `if (param_1 & 3)` on
//! each block's file-offset address and reads the stream via raw u32 loads, so an
//! unaligned block start aborts decompression -> no map renders.
//!
//! # Per block
//! A single LSB-first bit stream. Bit 0 = LSB of byte 0.
//! ```text
//! [info_size][raw_out][2b table_idx] then the code stream.
//! out_size = block_size - raw_out   (the number of output bytes this block yields)
//! The literal pool begins at BYTE offset info_size in the block.
//! info_size = ceil((header_bits + code_bits)/8).
//! ```
//! Codes are variable-length; the code for entry `e` is value `code[e]` packed as
//! `width[e]` bits, LSB-first, back to back. cmd2 (COPY_BYTES) appends `amt_bits`
//! amount bits; cmd3 (COPY_PREV_BYTES) appends `amt_bits` amount bits then
//! `off_bits` offset bits.

use rayon::prelude::*;

// --- Code tables (cpr_tclCodeTable::vSetStandardTable) ---------------------

const CMD_COPY_BYTE: u8 = 1;
const CMD_COPY_BYTES: u8 = 2;
const CMD_COPY_PREV_BYTES: u8 = 3;

#[derive(Clone, Copy)]
struct Entry {
    cmd_type: u8,
    code: u16,     // u16_0 : the code value
    width: u8,     // u8_0  : code bit width
    amt_bits: u8,  // u8_1  : extra amount bits (cmd2/cmd3)
    amt_base: u16, // u16_1 : amount base
    off_bits: u8,  // u8_2  : extra offset bits (cmd3)
    off_base: u16, // u16_3 : offset base
}

// Args mirror the Python CodeTableEntry constructor order so the table literals
// below are a direct transcription (the two `_` fields are unused by the codec).
fn e(cmd_type: u8, code: u16, width: u8, amt_bits: u8, amt_base: u16, _u16_2: u16, off_bits: u8, off_base: u16, _u16_4: u16) -> Entry {
    Entry { cmd_type, code, width, amt_bits, amt_base, off_bits, off_base }
}

const TABLES_RAW: [[(u8, u16, u8, u8, u16, u16, u8, u16, u16); 9]; 4] = [
    // table 0
    [
        (1, 0, 2, 0, 1, 1, 0, 0, 0),
        (3, 1, 2, 2, 2, 5, 4, 2, 32),
        (3, 2, 3, 2, 2, 5, 11, 546, 4640),
        (3, 3, 3, 2, 2, 5, 8, 34, 544),
        (2, 6, 3, 3, 2, 9, 0, 0, 0),
        (3, 7, 4, 5, 6, 37, 4, 2, 32),
        (3, 15, 5, 5, 6, 37, 8, 34, 544),
        (3, 31, 6, 5, 6, 37, 11, 546, 4640),
        (2, 63, 6, 8, 10, 265, 0, 0, 0),
    ],
    // table 1
    [
        (1, 0, 2, 0, 1, 1, 0, 0, 0),
        (3, 1, 2, 2, 2, 5, 3, 4, 32),
        (3, 2, 3, 2, 2, 5, 10, 548, 4640),
        (3, 3, 3, 2, 2, 5, 7, 36, 544),
        (2, 6, 3, 3, 2, 9, 0, 0, 0),
        (3, 7, 4, 5, 6, 37, 3, 4, 32),
        (3, 15, 5, 5, 6, 37, 7, 36, 544),
        (3, 31, 6, 5, 6, 37, 10, 548, 4640),
        (2, 63, 6, 8, 10, 265, 0, 0, 0),
    ],
    // table 2
    [
        (1, 0, 2, 0, 1, 1, 0, 0, 0),
        (3, 1, 2, 2, 2, 5, 4, 4, 64),
        (3, 2, 3, 2, 2, 5, 11, 1092, 9184),
        (3, 3, 3, 2, 2, 5, 8, 68, 1088),
        (2, 6, 3, 3, 2, 9, 0, 0, 0),
        (3, 7, 4, 4, 6, 21, 4, 4, 64),
        (3, 15, 5, 4, 6, 21, 8, 68, 1088),
        (3, 31, 6, 4, 6, 21, 11, 1092, 9184),
        (2, 63, 6, 7, 10, 137, 0, 0, 0),
    ],
    // table 3
    [
        (1, 0, 2, 0, 1, 1, 0, 0, 0),
        (3, 1, 2, 2, 2, 5, 4, 2, 32),
        (3, 2, 3, 2, 2, 5, 10, 546, 2592),
        (3, 3, 3, 2, 2, 5, 8, 34, 544),
        (2, 6, 3, 3, 2, 9, 0, 0, 0),
        (3, 7, 4, 5, 6, 37, 4, 2, 32),
        (3, 15, 5, 5, 6, 37, 8, 34, 544),
        (3, 31, 6, 5, 6, 37, 10, 546, 2592),
        (2, 63, 6, 8, 10, 265, 0, 0, 0),
    ],
];

// entry_10 >> 1 per table, used in the COPY_PREV offset:
// back = off_base + (off_extra << unknown_11)
const UNKNOWN_11: [u32; 4] = [1, 2, 2, 1];

struct CodeTable {
    entries: Vec<Entry>,
    lookup: Vec<u8>, // entry_3: code value -> index into entries (decoder fast path)
    mask: u32,       // entry_4 = (1 << bits) - 1
}

fn build_table(idx: usize) -> CodeTable {
    let raw = &TABLES_RAW[idx];
    let mut entries = Vec::with_capacity(raw.len());
    let mut max_bits = 0u32;
    for t in raw.iter() {
        let en = e(t.0, t.1, t.2, t.3, t.4, t.5, t.6, t.7, t.8);
        if (en.width as u32) > max_bits {
            max_bits = en.width as u32;
        }
        entries.push(en);
    }
    // vUpdateReferenceList: only the decoder needs the reverse lookup; the encoder
    // uses `entries` directly. Building it for every table costs at most 64 bytes.
    let total = 1u32 << max_bits;
    let mut lookup = vec![0u8; total as usize];
    for (i, en) in entries.iter().enumerate() {
        let stride = 1u32 << en.width;
        let mut v = en.code as u32;
        while v < total {
            lookup[v as usize] = i as u8;
            v += stride;
        }
    }
    CodeTable { entries, lookup, mask: total - 1 }
}

fn code_tables() -> [CodeTable; 4] {
    [build_table(0), build_table(1), build_table(2), build_table(3)]
}

// ============================================================================
// COMPRESSION
// ============================================================================

/// Options for [`compress`]. Defaults match the shipped CLI: 0x4000-byte blocks,
/// auto code-table selection, LZ77 on, best (level 9) parsing.
#[derive(Clone, Copy, Debug)]
pub struct CompressOptions {
    /// Block size in KiB. 16 -> 0x4000-byte blocks (MAP/IDX), 64 -> 0x10000 (LID).
    pub block_size_kib: u16,
    /// Force a single code table (0..3). `None` = pick the best per block.
    pub table: Option<usize>,
    /// Disable LZ77 back references (emit literals only).
    pub literal_only: bool,
    /// 1-4 fast greedy; 5-9 optimal-parsing DP (5->128 … 9->2048 hash window).
    pub level: u32,
}

impl Default for CompressOptions {
    fn default() -> Self {
        CompressOptions { block_size_kib: 16, table: None, literal_only: false, level: 9 }
    }
}

// --- Bit writer (inverse of the decompressor's LSB-first BitReader) --------

struct BitWriter {
    bytes: Vec<u8>,
    bitpos: u32, // total bits written so far
}

impl BitWriter {
    fn new() -> Self {
        BitWriter { bytes: Vec::new(), bitpos: 0 }
    }
    // Append `nbits` low bits of `value`, LSB-first. Highly optimized to write byte-wise.
    fn push(&mut self, value: u32, nbits: u32) {
        if nbits == 0 { return; }
        let mut val = value as u64;
        let mut bits_left = nbits;
        let mut pos = self.bitpos as usize;

        let needed_bytes = (pos + bits_left as usize + 7) / 8;
        if self.bytes.len() < needed_bytes {
            self.bytes.resize(needed_bytes, 0);
        }

        while bits_left > 0 {
            let byte_idx = pos >> 3;
            let bit_idx = pos & 7;

            let can_write = (8 - bit_idx).min(bits_left as usize);
            let mask = (1u64 << can_write) - 1;
            let chunk = (val & mask) as u8;

            self.bytes[byte_idx] |= chunk << bit_idx;

            val >>= can_write;
            pos += can_write;
            bits_left -= can_write as u32;
        }
        self.bitpos = pos as u32;
    }
    fn finish(mut self) -> Vec<u8> {
        let total = ((self.bitpos + 7) / 8) as usize;
        self.bytes.resize(total, 0);
        self.bytes
    }
}

// --- Symbols ---------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Sym {
    LitByte,             // COPY_BYTE: one pool byte
    LitRun { len: u32 }, // COPY_BYTES: `len` pool bytes
    BackRef { len: u32, dist: u32 }, // COPY_PREV_BYTES
}

// Pick the entry for a literal run length; None if it fits no cmd2 entry.
fn litrun_entry(table: &CodeTable, len: u32) -> Option<usize> {
    table.entries.iter().position(|en| {
        en.cmd_type == CMD_COPY_BYTES
            && (len as u16) >= en.amt_base
            && (len as u16) < en.amt_base + (1u16 << en.amt_bits)
    })
}

// Pick the entry for a back reference; None if it fits no cmd3 entry.
fn backref_entry(table: &CodeTable, len: u32, dist: u32, u11: u32) -> Option<usize> {
    table.entries.iter().position(|en| {
        if en.cmd_type != CMD_COPY_PREV_BYTES {
            return false;
        }
        let len_ok = (len as u16) >= en.amt_base && (len as u16) < en.amt_base + (1u16 << en.amt_bits);
        if !len_ok {
            return false;
        }
        // dist = off_base + (off_extra << u11), 0 <= off_extra < 2^off_bits.
        // So (dist - off_base) must be a multiple of 2^u11 and fit in off_bits after the shift.
        let d = dist as u32;
        let base = en.off_base as u32;
        if d < base {
            return false;
        }
        let step = 1u32 << u11; // 2^u11
        let rem = d - base;
        if rem % step != 0 {
            return false;
        }
        let off_extra = rem / step;
        off_extra < (1u32 << en.off_bits)
    })
}

fn sym_bitlen(s: &Sym, table: &CodeTable, u11: u32) -> u32 {
    match s {
        Sym::LitByte => {
            let en = &table.entries[table.entries.iter().position(|x| x.cmd_type == CMD_COPY_BYTE).unwrap()];
            en.width as u32
        }
        Sym::LitRun { len } => {
            let i = litrun_entry(table, *len).expect("litrun fits no entry");
            table.entries[i].width as u32 + table.entries[i].amt_bits as u32
        }
        Sym::BackRef { len, dist } => {
            let i = backref_entry(table, *len, *dist, u11).expect("backref fits no entry");
            let en = &table.entries[i];
            en.width as u32 + en.amt_bits as u32 + en.off_bits as u32
        }
    }
}

fn write_sym(bw: &mut BitWriter, s: &Sym, table: &CodeTable, u11: u32) {
    match s {
        Sym::LitByte => {
            let i = table.entries.iter().position(|x| x.cmd_type == CMD_COPY_BYTE).unwrap();
            let en = &table.entries[i];
            bw.push(en.code as u32, en.width as u32);
        }
        Sym::LitRun { len } => {
            let i = litrun_entry(table, *len).unwrap();
            let en = &table.entries[i];
            bw.push(en.code as u32, en.width as u32);
            bw.push((len - en.amt_base as u32) as u32, en.amt_bits as u32);
        }
        Sym::BackRef { len, dist } => {
            let i = backref_entry(table, *len, *dist, u11).unwrap();
            let en = &table.entries[i];
            bw.push(en.code as u32, en.width as u32);
            bw.push((len - en.amt_base as u32) as u32, en.amt_bits as u32);
            let off_extra = (*dist - en.off_base as u32) >> u11; // (d-base)/2^u11
            bw.push(off_extra, en.off_bits as u32);
        }
    }
}

// --- LZ77 symbol selection -------------------------------------------------

const HASH_BITS: usize = 16;
const HASH_SIZE: usize = 1 << HASH_BITS;
const MAX_CHAIN: usize = 24; // candidates walked per position (speed vs ratio)

// Seed on TWO bytes. Indexing by a 3-byte prefix missed every length-2 repeat, so short
// repeats were emitted as literals — Bosch instead spends ~60k tiny copies/file on dense MAP
// data and lands at far fewer literal bytes. A 2-byte seed finds them.
#[inline]
fn h2(b: &[u8], j: usize) -> usize {
    let v = (b[j] as u32) | ((b[j + 1] as u32) << 8);
    ((v.wrapping_mul(0x9E37_79B1)) >> (32 - HASH_BITS)) as usize
}

// Parity-split hash chains. Two head tables (one per position parity) share a single prev[]
// array: position j is linked only to earlier positions q with q % 2 == j % 2, so following the
// chain from p always yields an EVEN distance (p - q). Every COPY_PREV distance in these code
// tables is a multiple of 2 (off_base is even and the extra field is shifted by u11 >= 1), so an
// odd-distance match can never be encoded — restricting candidates to same-parity positions here
// keeps find_match/collect_matches from proposing dead matches instead of dropping them.
#[inline]
fn build_chains(chunk: &[u8]) -> Vec<usize> {
    let n = chunk.len();
    let mut head = vec![usize::MAX; 2 * HASH_SIZE];
    let mut prev = vec![usize::MAX; n];
    for j in 0..n.saturating_sub(1) {
        let h = h2(chunk, j) + (j & 1) * HASH_SIZE;
        prev[j] = head[h];
        head[h] = j;
    }
    prev
}

// Best match using flat prev array with vectorized memcmp
fn find_match(chunk: &[u8], prev: &[usize], p: usize, max_len: u32, max_dist: u32) -> Option<(u32, u32)> {
    let n = chunk.len();
    if n - p < 2 {
        return None;
    }
    let max_len = (max_len as usize).min(n - p);
    if max_len < 2 {
        return None;
    }
    let min_q = if (p as u32) > max_dist { p - max_dist as usize } else { 0 };

    let mut best_l = 1;
    let mut best_q = 0;
    let mut q = prev[p];
    let mut depth = 0;

    while q != usize::MAX && q >= min_q && depth < MAX_CHAIN {
        let cap = max_len.min(p - q);
        let mut l = 0;

        // Fast vectorized 8-byte compare
        while l + 8 <= cap {
            let a = unsafe { chunk.as_ptr().add(p + l).cast::<u64>().read_unaligned() };
            let b = unsafe { chunk.as_ptr().add(q + l).cast::<u64>().read_unaligned() };
            if a != b {
                l += (a ^ b).trailing_zeros() as usize / 8;
                break;
            }
            l += 8;
        }
        while l < cap && chunk[p + l] == chunk[q + l] {
            l += 1;
        }

        if l >= 2 && l > best_l {
            best_l = l;
            best_q = q;
            if l == max_len {
                break;
            }
        }

        q = prev[q];
        depth += 1;
    }

    if best_l >= 2 {
        Some((best_l as u32, (p - best_q) as u32))
    } else {
        None
    }
}

fn lz77(chunk: &[u8], table_idx: usize) -> (Vec<Sym>, Vec<u8>) {
    let tables = code_tables();
    let table = &tables[table_idx];
    let u11 = UNKNOWN_11[table_idx];

    let max_ref_len = table
        .entries
        .iter()
        .filter(|e| e.cmd_type == CMD_COPY_PREV_BYTES)
        .map(|e| e.amt_base as u32 + (1u32 << e.amt_bits) - 1)
        .max()
        .unwrap_or(0);
    let max_dist = table
        .entries
        .iter()
        .filter(|e| e.cmd_type == CMD_COPY_PREV_BYTES)
        .map(|e| e.off_base as u32 + (((1u32 << e.off_bits) - 1) << u11))
        .max()
        .unwrap_or(0);

    // Parity-split chains (see build_chains) so all candidate distances are even.
    let n = chunk.len();
    let prev = build_chains(chunk);

    let mut pool: Vec<u8> = Vec::new();
    let mut syms: Vec<Sym> = Vec::new();
    let mut stat_back = 0u64;
    let mut stat_lit = 0u64;

    let mut p = 0usize;
    while p < n {
        if let Some((len, dist)) = find_match(chunk, &prev, p, max_ref_len, max_dist) {
            if backref_entry(table, len, dist, u11).is_some() {
                syms.push(Sym::BackRef { len, dist });
                stat_back += len as u64;
                p += len as usize;
                continue;
            }
        }
        // Literal: gather a run until the next worthwhile match or 265 bytes.
        let mut run = 1usize;
        while p + run < n && run < 265 {
            let m = find_match(chunk, &prev, p + run, max_ref_len, max_dist);
            let worth = match m {
                Some((l, d)) => backref_entry(table, l, d, u11).is_some() && l >= 2,
                None => false,
            };
            if worth {
                break;
            }
            run += 1;
        }
        let mut r = run as u32;
        while r > 1 && litrun_entry(table, r).is_none() {
            r -= 1;
        }
        if r == 1 {
            syms.push(Sym::LitByte);
            pool.push(chunk[p]);
            stat_lit += 1;
            p += 1;
        } else {
            syms.push(Sym::LitRun { len: r });
            for k in 0..r as usize {
                pool.push(chunk[p + k]);
            }
            stat_lit += r as u64;
            p += r as usize;
        }
    }

    if std::env::var("CPR_STATS").is_ok() {
        let tot = stat_back + stat_lit;
        eprintln!(
            "  [stats] n={} backref_bytes={} ({:.1}%) literal_bytes={}",
            n,
            stat_back,
            if tot > 0 { 100.0 * stat_back as f64 / tot as f64 } else { 0.0 },
            stat_lit
        );
    }

    (syms, pool)
}

// --- Optimal-parsing LZ77 ("best" level) ----------------------------------

const LITRUN_CANDIDATES: &[usize] =
    &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128];

// Collect matches ensuring Pareto-optimality (each subsequent match must be strictly longer).
fn collect_matches(chunk: &[u8], prev: &[usize], p: usize, max_len: u32, max_dist: u32, chain_cap: usize) -> Vec<(u32, u32)> {
    let n = chunk.len();
    let mut out: Vec<(u32, u32)> = Vec::new();
    if n - p < 2 {
        return out;
    }
    let max_len = (max_len as usize).min(n - p);
    if max_len < 2 {
        return out;
    }
    let min_q = if (p as u32) > max_dist { p - max_dist as usize } else { 0 };

    let mut best_len = 1;
    let mut q = prev[p];
    let mut depth = 0;

    while q != usize::MAX && q >= min_q && depth < chain_cap {
        let cap = max_len.min(p - q); // overlap-safe (snapshot copy)
        let mut l = 0;

        // Vectorized 8-byte compare
        while l + 8 <= cap {
            let a = unsafe { chunk.as_ptr().add(p + l).cast::<u64>().read_unaligned() };
            let b = unsafe { chunk.as_ptr().add(q + l).cast::<u64>().read_unaligned() };
            if a != b {
                l += (a ^ b).trailing_zeros() as usize / 8;
                break;
            }
            l += 8;
        }
        while l < cap && chunk[p + l] == chunk[q + l] {
            l += 1;
        }

        if l > best_len {
            best_len = l;
            out.push((l as u32, (p - q) as u32));
            if l == max_len {
                break;
            }
        }

        q = prev[q];
        depth += 1;
    }
    out
}

fn lz77_best(chunk: &[u8], table_idx: usize, chain_cap: usize) -> (Vec<Sym>, Vec<u8>) {
    let tables = code_tables();
    let table = &tables[table_idx];
    let u11 = UNKNOWN_11[table_idx];

    let max_ref_len = table
        .entries
        .iter()
        .filter(|e| e.cmd_type == CMD_COPY_PREV_BYTES)
        .map(|e| e.amt_base as u32 + (1u32 << e.amt_bits) - 1)
        .max()
        .unwrap_or(0);
    let max_dist = table
        .entries
        .iter()
        .filter(|e| e.cmd_type == CMD_COPY_PREV_BYTES)
        .map(|e| e.off_base as u32 + (((1u32 << e.off_bits) - 1) << u11))
        .max()
        .unwrap_or(0);

    // Parity-split chains (see build_chains) so all candidate distances are even.
    let n = chunk.len();
    let prev = build_chains(chunk);

    // dp[p] = min (code_bits + 8*pool_bytes) to encode chunk[p..n].
    let mut dp = vec![u64::MAX; n + 1];
    dp[n] = 0;
    // choice at p: kind 0=litbyte, 1=litrun(a), 2=backref(a=len,b=dist)
    let mut kind = vec![0u8; n];
    let mut a = vec![0u32; n];
    let mut b = vec![0u32; n];

    for p in (0..n).rev() {
        let mut best: u64 = u64::MAX;
        // Literal moves.
        for &r in LITRUN_CANDIDATES {
            if r > n - p {
                continue;
            }
            let cbits: u32 = if r == 1 {
                2 // single COPY_BYTE
            } else {
                match litrun_entry(table, r as u32) {
                    Some(i) => table.entries[i].width as u32 + table.entries[i].amt_bits as u32,
                    None => continue,
                }
            };
            let c = cbits as u64 + 8 * (r as u64) + dp[p + r];
            if c < best {
                best = c;
                kind[p] = if r == 1 { 0 } else { 1 };
                a[p] = r as u32;
            }
        }
        // Back-reference moves.
        for (len, dist) in collect_matches(chunk, &prev, p, max_ref_len, max_dist, chain_cap) {
            let i = match backref_entry(table, len, dist, u11) {
                Some(i) => i,
                None => continue,
            };
            let en = &table.entries[i];
            let cbits = (en.width as u64) + (en.amt_bits as u64) + (en.off_bits as u64);
            let c = cbits + dp[p + len as usize];
            if c < best {
                best = c;
                kind[p] = 2;
                a[p] = len;
                b[p] = dist;
            }
        }
        dp[p] = best;
    }

    // Reconstruct the chosen symbol sequence.
    let mut syms: Vec<Sym> = Vec::new();
    let mut pool: Vec<u8> = Vec::new();
    let mut p = 0usize;
    while p < n {
        match kind[p] {
            0 => {
                syms.push(Sym::LitByte);
                pool.push(chunk[p]);
                p += 1;
            }
            1 => {
                let r = a[p] as usize;
                syms.push(Sym::LitRun { len: r as u32 });
                for k in 0..r {
                    pool.push(chunk[p + k]);
                }
                p += r;
            }
            _ => {
                let len = a[p] as usize;
                syms.push(Sym::BackRef { len: len as u32, dist: b[p] });
                p += len;
            }
        }
    }

    if std::env::var("CPR_STATS").is_ok() {
        let mut back = 0u64;
        let mut lit = 0u64;
        for s in &syms {
            match s {
                Sym::BackRef { len, .. } => back += *len as u64,
                Sym::LitByte => lit += 1,
                Sym::LitRun { len } => lit += *len as u64,
            }
        }
        let tot = back + lit;
        eprintln!(
            "  [stats] n={} backref_bytes={} ({:.1}%) literal_bytes={}",
            n,
            back,
            if tot > 0 { 100.0 * back as f64 / tot as f64 } else { 0.0 },
            lit
        );
    }

    (syms, pool)
}

// --- Block encoder ---------------------------------------------------------

fn encode_block(chunk: &[u8], table_idx: usize, block_size: usize, literal_only: bool, level: u32) -> Vec<u8> {
    let tables = code_tables();
    let table = &tables[table_idx];
    let u11 = UNKNOWN_11[table_idx];

    let (syms, pool): (Vec<Sym>, Vec<u8>) = if literal_only {
        let s: Vec<Sym> = vec![Sym::LitByte; chunk.len()];
        (s, chunk.to_vec())
    } else if level >= 5 {
        // "Best": optimal-parsing DP with a large hash window. The window is the
        // dominant ratio lever (long-range repeats need many candidates searched),
        // so it grows ~doubling per level: 5->128 ... 9->2048.
        let mut chain_cap = 128usize << (level as u32 - 5);
        if let Ok(v) = std::env::var("CPR_CHAIN") {
            if let Ok(c) = v.parse::<usize>() {
                chain_cap = c; // experimentation override
            }
        }
        lz77_best(chunk, table_idx, chain_cap)
    } else {
        lz77(chunk, table_idx)
    };

    // vInterpreteHeader: size fields are WORDs (16b) for block_size < 0x10000
    // and DWORDs (32b) otherwise. Must match the firmware/decompressor exactly.
    let size_bits = if block_size >= 0x10000 { 32u32 } else { 16u32 };
    let mut code_bits: u32 = size_bits * 2 + 2; // info_size + raw_out + table_idx
    for s in &syms {
        code_bits += sym_bitlen(s, table, u11);
    }
    let info_size = ((code_bits + 7) / 8) as u32;

    let out_size = chunk.len() as u32;
    let raw_out = block_size as u32 - out_size;

    let mut bw = BitWriter::new();
    bw.push(info_size, size_bits);
    bw.push(raw_out, size_bits);
    bw.push(table_idx as u32, 2);
    for s in &syms {
        write_sym(&mut bw, s, table, u11);
    }
    let mut block = bw.finish();
    if block.len() != info_size as usize {
        panic!("info_size mismatch: wrote {} bytes, expected {}", block.len(), info_size);
    }
    block.extend_from_slice(&pool);
    block
}

// Pad a compressed block up to the next multiple of 4 bytes.
//
// The firmware reader (DAPIAPP.OUT cpr_tclDecompressAlgorithm::bDecompressData) is fed each
// block by address, taken straight from the file's block-offset table
// (cpr_tclSectionDecompress::bDecompress → param_1 = fileBase + u32GetBlockBeginFileOffset(i)).
// It GUARDS that address: `if ((param_1 & 3) != 0)` → trace "CPR read access to unaligned adress"
// and bDecompressData returns failure. u32GetNextBits then reads the bit stream as a run of
// raw little-endian DWORDs (*puVar4, ptr += 1), which on this ARM target faults/misreads if the
// base is not 4-byte aligned. Since block i+1 begins exactly where block i ends (begin(i+1) =
// end(i)+1 semantics collapse to [begin_i, begin_{i+1})), every block start is 4-aligned IFF every
// compressed block length is a multiple of 4. Bosch does exactly this (all its blocks are mult-of-4;
// the offset table and `first` are too). Without the pad our blocks land on odd offsets → the nav
// rejects them and renders no map. Up to 3 zero bytes are appended inside the block's own region and
// are never read by the decoder (it stops at out_size / literal-pool consumption).
fn pad4(mut b: Vec<u8>) -> Vec<u8> {
    let rem = b.len() & 3;
    if rem != 0 {
        b.resize(b.len() + (4 - rem), 0);
    }
    b
}

// Encode one block with the best of the 4 code tables (each block's 2-bit
// table_idx is independent, so per-block selection is valid). If `table` is
// Some(t), only that table is used. PARALLEL: tries all 4 tables concurrently.
fn encode_block_best(chunk: &[u8], block_size: usize, table: Option<usize>, literal_only: bool, level: u32) -> Vec<u8> {
    let tables: Vec<usize> = if let Some(t) = table { vec![t] } else { vec![0, 1, 2, 3] };

    // Parallel table selection (4 independent tasks)
    let results: Vec<(usize, Vec<u8>)> = tables
        .into_par_iter()
        .map(|t| (t, encode_block(chunk, t, block_size, literal_only, level)))
        .collect();

    results.into_iter()
        .min_by_key(|(_, b)| b.len())
        .map(|(_, b)| b)
        .unwrap()
}

// Encode an all-zero block as a zero-cost "empty" block: info_size = header bytes, raw_out =
// block_size (=> out_size 0). size_bits is 16 for block_size < 0x10000 else 32, matching
// vInterpreteHeader. For block_size=0x4000 this yields the exact Bosch pattern 04 00 00 40:
// push(info_size=4,16) then push(raw_out=0x4000,16), LSB-first.
fn empty_block_bytes(block_size: usize) -> Vec<u8> {
    let size_bits = if block_size >= 0x10000 { 32u32 } else { 16u32 };
    let info_size = (2 * size_bits / 8) as u32;
    let mut bw = BitWriter::new();
    bw.push(info_size, size_bits);
    bw.push(block_size as u32, size_bits);
    bw.finish()
}

fn compress_with(data: &[u8], block_size_kib: u16, table: Option<usize>, literal_only: bool, level: u32) -> Vec<u8> {
    let block_size = (block_size_kib as usize) * 0x400;
    let n = data.len();
    // Contiguous tiling: full blocks of block_size, one partial at the end.
    let nblocks = if n == 0 { 1 } else { (n + block_size - 1) / block_size };

    // PARALLEL: compress all blocks concurrently, preserving order.
    // All-zero regions are emitted as "empty" blocks (raw_out = block_size => out_size 0),
    // costing only the size-field bytes; the runtime zero-fills that slot. This mirrors Bosch
    // and is what makes sparse data (.TCI/.TTC tile-cluster indices, tables) tiny. Without it
    // we spend ~1 KB/block copying zeros Bosch stores in ~4 bytes. Output placement is by block
    // INDEX (slot k*block_size), so a zero-cost empty block stays byte-correct on round-trip.
    let blocks: Vec<Vec<u8>> = (0..nblocks)
        .into_par_iter()
        .map(|k| {
            let start = k * block_size;
            let end = (start + block_size).min(n);
            let chunk = &data[start..end];
            let blk = if chunk.is_empty() || chunk.iter().all(|&b| b == 0) {
                empty_block_bytes(block_size)
            } else {
                encode_block_best(chunk, block_size, table, literal_only, level)
            };
            // Every compressed block must be a multiple of 4 bytes so the next block's file
            // offset (and thus bDecompressData's param_1) stays 4-byte aligned — see pad4.
            pad4(blk)
        })
        .collect();

    let first = 0x18usize + 4 * nblocks;
    let mut out = Vec::new();
    // header
    out.extend_from_slice(&5u16.to_le_bytes()); // version
    out.extend_from_slice(&block_size_kib.to_le_bytes()); // [0x02] block size in KiB
    out.extend_from_slice(b"CPRNAV_2");
    out.extend_from_slice(&(n as u32).to_le_bytes()); // unpacked_size
    out.push(3u8); // mode
    out.push(0u8); // [0x11]
    out.extend_from_slice(&1u16.to_le_bytes()); // [0x12] = 1
    assert_eq!(out.len(), 0x14);
    out.extend_from_slice(&(first as u32).to_le_bytes());
    let mut cur = first;
    for b in &blocks {
        cur += b.len();
        out.extend_from_slice(&(cur as u32).to_le_bytes()); // end offset
    }
    assert_eq!(out.len(), first);
    for b in &blocks {
        out.extend_from_slice(b);
    }
    out
}

/// Compress a buffer into a `CPRNAV_2` archive. See the module docs for the layout
/// and [`CompressOptions`] for the knobs. The result round-trips through
/// [`decompress`] byte-exactly.
pub fn compress(data: &[u8], opts: CompressOptions) -> Vec<u8> {
    compress_with(data, opts.block_size_kib, opts.table, opts.literal_only, opts.level)
}

// ============================================================================
// DECOMPRESSION
// ============================================================================

// --- Bit reader (cpr_tclDecompressAlgorithm::u32GetNextBits) --------------
// LSB-first reader over little-endian DWORDs. Faithful port: returns `n` bits
// positioned in the TOP of a u32; callers shift right as needed. State and
// operation order match the reference exactly (do not "clean up").

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    curr_dword: u32,
    bit_pos: u32,   // curr_dword_bit_pos
    remainder: u32, // dword_remainder
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        let mut br = BitReader { data, pos: 0, curr_dword: 0, bit_pos: 0, remainder: 0 };
        br.curr_dword = br.read_u32();
        br
    }

    fn read_u32(&mut self) -> u32 {
        if self.pos + 4 > self.data.len() {
            return 0; // reference reads 0 past end-of-block
        }
        let v = u32::from_le_bytes([
            self.data[self.pos],
            self.data[self.pos + 1],
            self.data[self.pos + 2],
            self.data[self.pos + 3],
        ]);
        self.pos += 4;
        v
    }

    fn next_bits(&mut self, n: u32) -> u32 {
        if n == 0 || n > 32 {
            panic!("bad bit count {}", n);
        }
        let mut bit_pos = self.bit_pos;

        if bit_pos != 0 {
            let some_bool = n >= 32 - bit_pos;
            if n <= 32 - bit_pos {
                let dword_remainder = self.remainder;
                if some_bool {
                    // n == 32 - bit_pos: consume exactly the remaining bits.
                    self.bit_pos = 0;
                    return dword_remainder;
                } else {
                    bit_pos += n;
                    let next_bits = dword_remainder << (32 - bit_pos);
                    self.remainder = dword_remainder ^ (next_bits >> (32 - bit_pos));
                    self.bit_pos = bit_pos;
                    return next_bits;
                }
            } else {
                let some_bits = n - (32 - bit_pos);
                let curr_dword = self.curr_dword;
                self.bit_pos = some_bits;
                let old_remainder = curr_dword << (32 - some_bits);
                let new_remainder = (curr_dword >> some_bits) << some_bits;
                let dword_remainder = self.remainder;
                self.curr_dword = self.read_u32();
                self.remainder = new_remainder;
                return old_remainder | (dword_remainder >> some_bits);
            }
        } else {
            if n == 32 {
                let next_bits = self.curr_dword;
                self.curr_dword = self.read_u32();
                return next_bits;
            } else {
                let remaining = 32 - n;
                let next_bits = self.curr_dword << remaining;
                self.remainder = self.curr_dword ^ (next_bits >> remaining);
                self.curr_dword = self.read_u32();
                self.bit_pos = n;
                return next_bits;
            }
        }
    }
}

// --- Per-block decompression ---------------------------------------------

fn unpack_block(block: &[u8], tables: &[CodeTable; 4], block_size: usize) -> Vec<u8> {
    let mut br = BitReader::new(block);

    // vInterpreteHeader: size fields are WORDs for block_size < 0x10000, DWORDs otherwise.
    let (info_size, raw_out): (u32, u32) = if block_size >= 0x10000 {
        (br.next_bits(32), br.next_bits(32))
    } else {
        (br.next_bits(16) >> 16, br.next_bits(16) >> 16)
    };

    let out_size = block_size - raw_out as usize;
    let mut out = vec![0u8; out_size];
    if std::env::var("CPR_DEBUG").is_ok() {
        eprintln!("  [block] compr_len={:#x} info_size={:#x} raw_out={:#x} out_size={:#x}", block.len(), info_size, raw_out, out_size);
    }

    let mut file_pointer = info_size as usize; // literal cursor into the block bytes
    let unknown_11: u32;
    let table_idx;
    let mut info_bytes: u32;
    {
        info_bytes = br.next_bits(32);
        table_idx = (info_bytes & 3) as usize;
        info_bytes >>= 2;
        unknown_11 = UNKNOWN_11[table_idx];
    }
    let table = &tables[table_idx];

    let mut num_bits: u32 = 2;
    let mut write_address: usize = 0;

    loop {
        // Inner loop handles repeated COPY_PREV (cmd 3) runs. `code_entry` is the
        // entry that breaks out of the inner loop (the first non-cmd-3 code); it is
        // reused by the outer handling below and must NOT be re-read here, because
        // info_bytes has already been shifted past it.
        let mut code_entry: &Entry;
        loop {
            if write_address == out_size {
                if std::env::var("CPR_DEBUG").is_ok() {
                    eprintln!("  [block] wrote={:#x} file_pointer_end={:#x} (pool used={:#x})", write_address, file_pointer, file_pointer - info_size as usize);
                }
                return out;
            }
            let entry_index = table.lookup[(info_bytes & table.mask) as usize];
            code_entry = &table.entries[entry_index as usize];

            num_bits += code_entry.width as u32;
            info_bytes >>= code_entry.width;

            if code_entry.cmd_type != CMD_COPY_PREV_BYTES {
                break;
            }

            let next_bits = br.next_bits(num_bits);
            info_bytes |= next_bits;

            let amt = (code_entry.amt_base as usize) + ((info_bytes & ((1u32 << code_entry.amt_bits) - 1)) as usize);
            info_bytes >>= code_entry.amt_bits;

            let back = (code_entry.off_base as u32) + ((info_bytes & ((1u32 << code_entry.off_bits) - 1)) << unknown_11);
            info_bytes >>= code_entry.off_bits;

            let amt = amt.min(out_size - write_address);
            let src = write_address - back as usize;
            out.copy_within(src..src + amt, write_address);
            write_address += amt;

            num_bits = code_entry.off_bits as u32 + code_entry.amt_bits as u32;
        }

        // Outer: COPY_BYTES (cmd 2) or COPY_BYTE (cmd 1), using the breaking entry.
        if code_entry.cmd_type == CMD_COPY_BYTES {
            let next_bits = br.next_bits(num_bits);
            info_bytes |= next_bits;

            let amt = (code_entry.amt_base as usize) + ((info_bytes & ((1u32 << code_entry.amt_bits) - 1)) as usize);
            info_bytes >>= code_entry.amt_bits;
            num_bits = code_entry.amt_bits as u32;

            let amt = amt.min(out_size - write_address);
            out[write_address..write_address + amt].copy_from_slice(&block[file_pointer..file_pointer + amt]);
            write_address += amt;
            file_pointer += amt;
        } else if code_entry.cmd_type == CMD_COPY_BYTE {
            if file_pointer >= block.len() {
                panic!("literal over-read at write={:#x}/out={:#x} fp={}", write_address, out_size, file_pointer);
            }
            out[write_address] = block[file_pointer];
            write_address += 1;
            file_pointer += 1;
        } else {
            panic!("bad cmd_type {} at write={:#x}", code_entry.cmd_type, write_address);
        }
    }
}

// --- Header + driver ------------------------------------------------------

struct Header {
    block_size: usize,
    unpacked_size: usize,
    blocks: Vec<(usize, usize)>,
}

fn parse_header(data: &[u8]) -> Result<Header, String> {
    fn u16(d: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([d[o], d[o + 1]])
    }
    fn u32(d: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
    }

    if data.len() < 0x14 {
        return Err("file too small".into());
    }
    let version = u16(data, 0);
    if version != 5 {
        return Err(format!("invalid version (expected 5, got {})", version));
    }
    let block_size_kib = u16(data, 2) as usize;
    if block_size_kib > 64 {
        return Err(format!("invalid block_size_kib (<=64, got {})", block_size_kib));
    }
    if &data[4..12] != b"CPRNAV_2" {
        return Err(format!(
            "bad signature {:?} (expected \"CPRNAV_2\")",
            String::from_utf8_lossy(&data[4..12])
        ));
    }

    let block_size = block_size_kib * 0x400; // bytes
    let unpacked_size = u32(data, 12) as usize;
    let compression_mode = u16(data, 16);
    if compression_mode != 3 {
        return Err(format!("invalid mode (expected 3, got {})", compression_mode));
    }

    // Block-end offsets: read DWORDs from 0x14 until reaching first_block_offset.
    let mut pos = 0x14;
    let first = u32(data, pos) as usize;
    pos += 4;
    let mut ends: Vec<usize> = Vec::new();
    while pos < first {
        if pos + 4 > data.len() {
            return Err("truncated block-offset table".into());
        }
        ends.push(u32(data, pos) as usize);
        pos += 4;
    }

    let mut blocks = Vec::with_capacity(ends.len());
    for (i, &end) in ends.iter().enumerate() {
        let start = if i == 0 { first } else { ends[i - 1] };
        blocks.push((start, end));
    }

    Ok(Header { block_size, unpacked_size, blocks })
}

/// Decompress a `CPRNAV_2` archive. Returns the unpacked buffer, or a `String`
/// describing why the input is not a valid archive. Handles both 16-bit
/// (MAP/IDX, block_size_kib=16) and 32-bit (LID, block_size_kib=64) size fields.
pub fn decompress(data: &[u8]) -> Result<Vec<u8>, String> {
    let hdr = parse_header(data)?;
    let tables = code_tables();
    // Output buffer is pre-zeroed. Each block k maps to the FIXED slot k*block_size and
    // produces `out_size` bytes there (0..=block_size); any region a block does not cover
    // stays zero. Bosch relies on this: all-zero regions are stored as empty blocks
    // (raw_out = block_size, out_size = 0) that cost only a few file bytes. Placing blocks by
    // running offset + pad is wrong whenever an empty/short block appears mid-stream.
    let mut out = vec![0u8; hdr.unpacked_size];

    for (bi, (start, end)) in hdr.blocks.iter().enumerate() {
        if std::env::var("CPR_DEBUG").is_ok() {
            eprintln!("[block {}] range=[{:#x},{:#x}) compr_len={:#x}", bi, start, end, end - start);
        }
        if *end > data.len() {
            return Err(format!("block [{:#x},{:#x}] exceeds file size {}", start, end, data.len()));
        }
        let block = &data[*start..*end];
        let unpacked = unpack_block(block, &tables, hdr.block_size);
        let base = bi * hdr.block_size;
        if base < hdr.unpacked_size {
            let n = unpacked.len().min(hdr.unpacked_size - base);
            out[base..base + n].copy_from_slice(&unpacked[..n]);
        }
    }

    Ok(out)
}

/// Cheap content sniff: does `data` begin with a `CPRNAV_2` header
/// (version 5, magic `CPRNAV_2` at +4)? Used by the CLIs to skip non-archive
/// files and to guard the compressor against nesting.
pub fn is_cprnav(data: &[u8]) -> bool {
    data.len() >= 12
        && u16::from_le_bytes([data[0], data[1]]) == 5
        && &data[4..12] == b"CPRNAV_2"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(name: &str, data: &[u8], opts: CompressOptions) {
        let packed = compress(data, opts);
        assert!(is_cprnav(&packed), "{name}: compressed output is not CPRNAV_2");
        let back = decompress(&packed).unwrap_or_else(|e| panic!("{name}: decompress failed: {e}"));
        assert_eq!(back.len(), data.len(), "{name}: length differs");
        assert_eq!(back, data, "{name}: bytes differ");
    }

    #[test]
    fn empty() {
        roundtrip("empty", &[], CompressOptions::default());
    }

    #[test]
    fn tiny() {
        roundtrip("tiny", b"hello world, hello world", CompressOptions::default());
    }

    #[test]
    fn all_zero_spanning_blocks() {
        // Sparse/zero regions exercise the empty-block path and block-slot placement.
        let mut v = vec![0u8; 0x4000 * 3 + 123];
        for (i, b) in v.iter_mut().enumerate() {
            if i % 5000 < 3 {
                *b = (i as u8).wrapping_mul(7);
            }
        }
        roundtrip("sparse", &v, CompressOptions::default());
    }

    #[test]
    fn pseudo_random_64k_blocks() {
        // LID-style: block_size_kib=64 (32-bit size fields), hard-to-compress data.
        let mut s = 0x1234_5678u32;
        let mut v = Vec::with_capacity(200_000);
        for _ in 0..v.capacity() {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            v.push((s & 0xff) as u8);
        }
        roundtrip("random64", &v, CompressOptions { block_size_kib: 64, ..Default::default() });
    }

    #[test]
    fn every_level_and_table() {
        let v = b"THEQUICKBROWNFOXJUMPSOVERTHELAZYDOG ".repeat(400);
        for level in [1u32, 3, 5, 9] {
            for table in [Some(0usize), Some(1), Some(2), Some(3), None] {
                roundtrip(
                    &format!("rep/l{level}/t{table:?}"),
                    &v,
                    CompressOptions { level, table, ..Default::default() },
                );
            }
        }
    }

    #[test]
    fn literal_only() {
        let v: Vec<u8> = (0..5000).map(|i| (i % 251) as u8).collect();
        roundtrip("litonly", &v, CompressOptions { literal_only: true, ..Default::default() });
    }

    #[test]
    fn rejects_garbage() {
        assert!(decompress(&[0u8; 8]).is_err());
        assert!(decompress(b"not a cprnav file at all........").is_err());
    }
}
