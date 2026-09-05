// Bosch TravelMap (Nissan LCN2KAI) CPRNAV_2 COMPRESSOR.
// Inverse of cprnav_decompress_rs (which is a port of the firmware
// cpr_tclDecompressAlgorithm / cpr_tclFileheader). Produces files that both the
// Rust decompressor and the firmware expand to the exact input bytes.
//
// PARALLEL: uses rayon to compress blocks concurrently.
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
// Every block's stored length is a MULTIPLE OF 4 bytes (zero-padded, see pad4). Bosch does the same:
// DAPIAPP.OUT bDecompressData guards `if (param_1 & 3)` on each block's file-offset address and reads
// the stream via raw u32 loads, so an unaligned block start aborts decompression -> no map renders.
//
// Per-block: a single LSB-first bit stream. Bit 0 = LSB of byte 0.
//   [16b info_size][16b raw_out][2b table_idx] then the code stream.
//   out_size = block_size - raw_out   (the number of output bytes this block yields)
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
use rayon::prelude::*;

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

// --- File writer -----------------------------------------------------------

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

fn compress(data: &[u8], block_size_kib: u16, table: Option<usize>, literal_only: bool, level: u32) -> Vec<u8> {
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
             Defaults: level=9 (best), block-kib=16 (0x4000-byte blocks), auto code-table.\n\
             --level N      1-9. 1-4 fast greedy; 5-9 optimal-parsing (best) + bigger window\n\
             --no-lz        disable LZ77 back refs (literal only)\n\
             --block-kib K  block size in KiB: 16 -> 0x4000 bytes, 64 -> 0x10000 bytes\n\
             --table T      force code table 0..3 (default: auto-pick best per block)
             --force        compress even if the input already looks CPRNAV_2 (nested)\n\
             --force        compress even if the input already looks CPRNAV_2 (nested)\n\n\
             PARALLEL: Uses rayon to compress blocks and select tables concurrently."
        );
        exit(if args.is_empty() { 1 } else { 0 });
    }

    let mut literal_only = false;
    let mut block_size_kib: u16 = 16;
    let mut level: u32 = 9;
    let mut table: Option<usize> = None;
    let mut force = false;
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
            "--force" => force = true,
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
    // Guard against double-compression: these map files (notably *.TCI / *.IDX) are ALREADY
    // CPRNAV_2 on the card, so feeding a packed file straight into the compressor silently
    // produces a nested archive that decompresses to garbage and blanks the map. Refuse unless --force.
    if data.len() >= 12 && u16::from_le_bytes([data[0], data[1]]) == 5 && &data[4..12] == b"CPRNAV_2" {
        eprintln!(
            "error: {} already looks like a CPRNAV_2 archive (version=5, magic at +4). \
             Decompress it first (cprnav_decompress_rs) before compressing; the runtime cannot read nested files. \
             Use --force to override.",
            input.display()
        );
        if !force {
            exit(2);
        }
    }
    let packed = compress(&data, block_size_kib, table, literal_only, level);
    if let Err(e) = fs::write(&out, &packed) {
        eprintln!("write {}: {}", out.display(), e);
        exit(1);
    }
    println!("{:<40} {:>10} -> {:>10}  ({})", input.display(), data.len(), packed.len(), out.display());
}
