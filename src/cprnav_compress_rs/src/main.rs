// Bosch TravelMap (Nissan LCN2KAI) CPRNAV_2 COMPRESSOR.
// Inverse of cprnav_decompress_rs (which is a port of the firmware
// cpr_tclDecompressAlgorithm / cpr_tclFileheader). Produces files that both the
// Rust decompressor and the firmware expand to the exact input bytes.
//
// File layout (little-endian):
//   [0x00] u16 version = 5
//   [0x02] u16 block_size_kib  (block_size = block_size_kib * 0x400 bytes; 16 -> 0x4000, 64 -> 0x10000)
//   [0x04] "CPRNAV_2"
//   [0x0c] u32 unpacked_size
//   [0x10] u8  mode = 3
//   [0x11] u8  0
//   [0x12] u16 1
//   [0x14] u32 first = 0x18 + 4*nblocks   (offset of block data)
//   [0x18 + 4*i] u32 end offset of block i   (i in 0..nblocks)
//   [first .. ] block data
//
// Per-block: a single LSB-first bit stream. Bit 0 = LSB of byte 0.
//   [16b info_size][16b raw_out][2b table_idx] then the code stream.
//   out_size = block_size - raw_out  (the number of output bytes this block yields)
//   The literal pool begins at BYTE offset info_size in the block.
//   info_size = ceil((34 + code_bits)/8).
//
// Codes are variable-length; code for entry e is value u16_0[e] packed as u8_0[e]
// bits, LSB-first, back to back. cmd2 (COPY_BYTES) appends u8_1 amount bits;
// cmd3 (COPY_PREV_BYTES) appends u8_1 amount bits then u8_2 offset bits.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;

const CMD_COPY_BYTE: u8 = 1;
const CMD_COPY_BYTES: u8 = 2;
const CMD_COPY_PREV_BYTES: u8 = 3;

#[derive(Clone, Copy)]
struct Entry {
    cmd_type: u8,
    code: u16,   // u16_0 : the code value
    width: u8,   // u8_0  : code bit width
    amt_bits: u8,// u8_1  : extra amount bits (cmd2/cmd3)
    amt_base: u16, // u16_1
    off_bits: u8,// u8_2  : extra offset bits (cmd3)
    off_base: u16, // u16_3
}

fn e(cmd_type: u8, code: u16, width: u8, amt_bits: u8, amt_base: u16, _u16_2: u16, off_bits: u8, off_base: u16, _u16_4: u16) -> Entry {
    Entry { cmd_type, code, width, amt_bits, amt_base, off_bits, off_base }
}

// Same literals as the decompressor (cpr_tclCodeTable::vSetStandardTable).
const TABLES_RAW: [[(u8, u16, u8, u8, u16, u16, u8, u16, u16); 9]; 4] = [
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

// entry_10 >> 1 per table, used in the COPY_PREV offset: back = off_base + (off_extra << unknown_11)
const UNKNOWN_11: [u32; 4] = [1, 2, 2, 1];

struct CodeTable {
    entries: Vec<Entry>,
}

fn build_table(idx: usize) -> CodeTable {
    let raw = &TABLES_RAW[idx];
    let entries = raw.iter().map(|t| e(t.0, t.1, t.2, t.3, t.4, t.5, t.6, t.7, t.8)).collect();
    CodeTable { entries }
}

fn code_tables() -> [CodeTable; 4] {
    [build_table(0), build_table(1), build_table(2), build_table(3)]
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
    // Append `nbits` low bits of `value`, LSB-first (bit 0 of stream = LSB of byte 0).
    fn push(&mut self, value: u32, nbits: u32) {
        for i in 0..nbits {
            let bit = (value >> i) & 1;
            if bit == 1 {
                let b = (self.bitpos / 8) as usize;
                if self.bytes.len() <= b {
                    self.bytes.resize(b + 1, 0);
                }
                self.bytes[b] |= 1 << (self.bitpos % 8);
            }
            self.bitpos += 1;
        }
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

const HASH_BITS: usize = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const MAX_CHAIN: usize = 16; // candidates walked per position (speed vs ratio)

fn h3(b: &[u8], j: usize) -> usize {
    let v = (b[j] as u32) ^ ((b[j + 1] as u32) << 5) ^ ((b[j + 2] as u32) << 9);
    (v & ((HASH_SIZE - 1) as u32)) as usize
}

// Best match of chunk[p..] against earlier positions, overlap-safe (the source
// byte at an overlapped position is chunk[q+l], which equals the already-written
// output). Returns (len, dist) with len>=2, or None.
fn find_match(chunk: &[u8], chains: &[[usize; MAX_CHAIN]], chain_len: &[u16], p: usize, max_len: u32, max_dist: u32) -> Option<(u32, u32)> {
    // h3 needs 3 bytes; also need at least a 2-byte match to be worthwhile.
    if p < 1 || chunk.len() - p < 3 {
        return None;
    }
    let max_len = (max_len as usize).min(chunk.len() - p);
    if max_len < 2 {
        return None;
    }
    let min_q = if (p as u32) > max_dist { p - max_dist as usize } else { 0 };
    let h = h3(chunk, p);
    let cl = chain_len[h] as usize;
    let start = cl.saturating_sub(MAX_CHAIN);
    let mut best: Option<(usize, usize)> = None; // (len, q)
    for &q in &chains[h][start..cl] {
        if q >= p || q < min_q {
            continue;
        }
        // Non-overlapping only: the match source [q, q+l) must lie entirely in the
        // already-produced prefix [0, p). The decompressor copies with snapshot
        // semantics (not memmove), so overlapping refs would corrupt the output.
        let cap = max_len.min(p - q);
        let mut l = 0usize;
        while l < cap && chunk[p + l] == chunk[q + l] {
            l += 1;
        }
        if l >= 2 && best.map_or(true, |b| l > b.0) {
            best = Some((l, q));
            if l == max_len {
                break;
            }
        }
    }
    best.map(|(l, q)| (l as u32, (p - q) as u32))
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

    // Build hash chains over all 3-byte prefixes of the chunk.
    let mut chains: Vec<[usize; MAX_CHAIN]> = vec![[0; MAX_CHAIN]; HASH_SIZE];
    let mut chain_len: Vec<u16> = vec![0; HASH_SIZE];
    let n = chunk.len();
    for j in 0..n.saturating_sub(2) {
        let h = h3(chunk, j);
        let l = chain_len[h] as usize;
        if l < MAX_CHAIN {
            chains[h][l] = j;
            chain_len[h] = (l + 1) as u16;
        }
    }

    let mut pool: Vec<u8> = Vec::new();
    let mut syms: Vec<Sym> = Vec::new();
    let mut stat_back = 0u64;
    let mut stat_lit = 0u64;

    let mut p = 0usize;
    while p < n {
        if let Some((len, dist)) = find_match(chunk, &chains, &chain_len, p, max_ref_len, max_dist) {
            // Only take the reference if it actually saves bits vs literals.
            // A back ref costs width+amt_bits+off_bits; a literal byte costs 2 bits
            // (COPY_BYTE). Require len >= 2 and that the entry exists.
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
            let m = find_match(chunk, &chains, &chain_len, p + run, max_ref_len, max_dist);
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
//
// Greedy picking is suboptimal: a slightly shorter match + literals can be
// cheaper than the longest match, and literal runs should be batched to save
// code bits. This does a shortest-path (DP) over positions where every candidate
// move carries its exact bit cost from the fixed code table plus 8 bits per pool
// byte. Minimizing code_bits + 8*pool_bytes minimizes the block size (up to <1
// byte of ceil rounding). Lazy matching falls out for free: "emit a literal then
// take a longer match" is just another path the DP may prefer.

const LITRUN_CANDIDATES: &[usize] =
    &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128];

// Encodable back-reference matches at position p, most-recent source first
// (closer sources use cheaper offset codes). Returns up to 40 (len, dist).
fn collect_matches(chunk: &[u8], chains: &Vec<Vec<usize>>, p: usize, max_len: u32, max_dist: u32) -> Vec<(u32, u32)> {
    let n = chunk.len();
    if p < 1 || n - p < 3 {
        return Vec::new();
    }
    let max_len = (max_len as usize).min(n - p);
    if max_len < 2 {
        return Vec::new();
    }
    let min_q = if (p as u32) > max_dist { p - max_dist as usize } else { 0 };
    let cl = &chains[h3(chunk, p)];
    let mut out: Vec<(u32, u32)> = Vec::new();
    for &q in cl.iter().rev() {
        if q >= p || q < min_q {
            continue;
        }
        let cap = max_len.min(p - q); // overlap-safe (snapshot copy)
        let mut l = 0usize;
        while l < cap && chunk[p + l] == chunk[q + l] {
            l += 1;
        }
        if l >= 2 {
            out.push((l as u32, (p - q) as u32));
            if out.len() >= 40 {
                break;
            }
        }
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

    let n = chunk.len();
    // Hash chains over 3-byte prefixes, capped per bucket.
    let mut chains: Vec<Vec<usize>> = vec![Vec::new(); HASH_SIZE];
    for j in 0..n.saturating_sub(2) {
        let h = h3(chunk, j);
        let bkt = &mut chains[h];
        if bkt.len() < chain_cap {
            bkt.push(j);
        }
    }

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
        for (len, dist) in collect_matches(chunk, &chains, p, max_ref_len, max_dist) {
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

// --- File writer -----------------------------------------------------------

// Encode one block with the best of the 4 code tables (each block's 2-bit
// table_idx is independent, so per-block selection is valid). If `table` is
// Some(t), only that table is used.
fn encode_block_best(chunk: &[u8], block_size: usize, table: Option<usize>, literal_only: bool, level: u32) -> Vec<u8> {
    let tables = if let Some(t) = table { vec![t] } else { vec![0, 1, 2, 3] };
    let mut best: Option<Vec<u8>> = None;
    for t in tables {
        let b = encode_block(chunk, t, block_size, literal_only, level);
        if best.as_ref().map_or(true, |bb| b.len() < bb.len()) {
            best = Some(b);
        }
    }
    best.unwrap()
}

fn compress(data: &[u8], block_size_kib: u16, table: Option<usize>, literal_only: bool, level: u32) -> Vec<u8> {
    let block_size = (block_size_kib as usize) * 0x400;
    let n = data.len();
    // Contiguous tiling: full blocks of block_size, one partial at the end.
    let nblocks = if n == 0 { 1 } else { (n + block_size - 1) / block_size };

    let mut blocks: Vec<Vec<u8>> = Vec::with_capacity(nblocks);
    for k in 0..nblocks {
        let start = k * block_size;
        let end = (start + block_size).min(n);
        let chunk = &data[start..end];
        blocks.push(encode_block_best(chunk, block_size, table, literal_only, level));
    }

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

// --- CLI -------------------------------------------------------------------

fn default_out(input: &Path) -> PathBuf {
    let stem = input.file_stem().map(|s| s.to_os_string()).unwrap_or_default();
    let mut full = stem;
    full.push(".CPR");
    match input.parent() {
        Some(p) => p.join(full),
        None => PathBuf::from(full),
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0].eq_ignore_ascii_case("-h") || args[0] == "--help" {
        eprintln!(
            "Usage:\n  cprnav_compress_rs <file> [out] [--level N] [--block-kib K] [--table T]\n\n\
             Compress a file into CPRNAV_2 (inverse of cprnav_decompress_rs).\n\
             Defaults: level=1 (fast), block-kib=16 (0x4000-byte blocks), auto code-table.\n\
             --level N      1-9. 1-4 fast greedy; 5-9 optimal-parsing (best) + bigger window\n\
             --no-lz        disable LZ77 back refs (literal only)\n\
             --block-kib K  block size in KiB: 16 -> 0x4000 bytes, 64 -> 0x10000 bytes\n\
             --table T      force code table 0..3 (default: auto-pick best per block)"
        );
        exit(if args.is_empty() { 1 } else { 0 });
    }

    let mut literal_only = false;
    let mut block_size_kib: u16 = 16;
    let mut level: u32 = 1;
    let mut table: Option<usize> = None;
    let mut files: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lz" => literal_only = false,
            "--no-lz" => literal_only = true,
            "--level" => {
                i += 1;
                level = args[i].parse().expect("bad --level");
            }
            "--block-kib" => {
                i += 1;
                block_size_kib = args[i].parse().expect("bad --block-kib");
            }
            "--table" => {
                i += 1;
                table = Some(args[i].parse().expect("bad --table"));
            }
            a => files.push(a.to_string()),
        }
        i += 1;
    }

    if files.is_empty() {
        eprintln!("no input file");
        exit(1);
    }
    let input = Path::new(&files[0]);
    let out = match files.get(1) {
        Some(o) => PathBuf::from(o),
        None => default_out(input),
    };

    let data = match fs::read(input) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("read {}: {}", input.display(), e);
            exit(1);
        }
    };
    let packed = compress(&data, block_size_kib, table, literal_only, level);
    if let Err(e) = fs::write(&out, &packed) {
        eprintln!("write {}: {}", out.display(), e);
        exit(1);
    }
    println!("{:<40} {:>10} -> {:>10}  ({})", input.display(), data.len(), packed.len(), out.display());
}
