// Bosch TravelMap (Nissan LCN2KAI) CPRNAV_2 decompressor.
// Rust port of Firmware/tools/lcn2kai-decompress/DecompressAlgorithm.py, corrected
// against the firmware reference (DAPIAPP.OUT cpr_tclDecompressAlgorithm).
//
// The original Python tool only handled per-block headers with 16-bit size fields
// (block_size < 0x10000, e.g. MAP/IDX files where `unknown`=16 -> block_size=0x4000)
// and failed on every LID file (`unknown`=64 -> block_size=0x10000). The firmware
// (cpr_tclDecompressAlgorithm::vInterpreteHeader) reads the per-block info/out sizes
// as 32-bit DWORDs when block_size >= 0x10000 and as 16-bit WORDs otherwise. This
// port implements both paths, so it decompresses MAP/IDX and LID alike.
//
// Verified byte-exact against the known-good unpacked N1E10AA.IDX (unknown=16) and
// against the firmware algorithm on LID files (unknown=64).
//
// Usage:
//   cprnav_decompress_rs <file> [out]          decompress one file (out defaults to
//                                              <input_dir>/<basename>.BIN)
//   cprnav_decompress_rs <dir>  [outdir]       decompress every CPRNAV_2 file in dir
//                                              (outdir defaults to dir)

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;

const CMD_COPY_BYTE: u8 = 1;
const CMD_COPY_BYTES: u8 = 2;
const CMD_COPY_PREV_BYTES: u8 = 3;

// --- Code tables (cpr_tclCodeTable::vSetStandardTable) -------------------

#[derive(Clone, Copy)]
struct Entry {
    cmd_type: u8,
    u16_0: u16, // start code value (reference-list build)
    u8_0: u8,   // bit width for the code index
    u8_1: u8,   // extra bits for amount
    u16_1: u16, // amount base
    u8_2: u8,   // extra bits for offset
    u16_3: u16, // offset base
}

// Args mirror the Python CodeTableEntry constructor order so the table literals
// below are a direct transcription (u16_2/u16_4 are unused by the decoder).
fn e(cmd_type: u8, u16_0: u16, u8_0: u8, u8_1: u8, u16_1: u16, _u16_2: u16, u8_2: u8, u16_3: u16, _u16_4: u16) -> Entry {
    Entry { cmd_type, u16_0, u8_0, u8_1, u16_1, u8_2, u16_3 }
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

// entry_10 per table -> unknown_11 = entry_10 >> 1 (used in the COPY_PREV offset).
const UNKNOWN_11: [u32; 4] = [1, 2, 2, 1];

struct CodeTable {
    entries: Vec<Entry>,
    lookup: Vec<u8>, // entry_3: code value -> index into entries
    mask: u32,       // entry_4 = (1 << bits) - 1
}

fn build_table(idx: usize) -> CodeTable {
    let raw = &TABLES_RAW[idx];
    let mut entries = Vec::with_capacity(raw.len());
    let mut max_bits = 0u32;
    for t in raw.iter() {
        let en = e(t.0, t.1, t.2, t.3, t.4, t.5, t.6, t.7, t.8);
        if (en.u8_0 as u32) > max_bits {
            max_bits = en.u8_0 as u32;
        }
        entries.push(en);
    }
    // vUpdateReferenceList
    let total = 1u32 << max_bits;
    let mut lookup = vec![0u8; total as usize];
    for (i, en) in entries.iter().enumerate() {
        let stride = 1u32 << en.u8_0;
        let mut v = en.u16_0 as u32;
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

// --- Bit reader (cpr_tclDecompressAlgorithm::u32GetNextBits) --------------
// LSB-first reader over little-endian DWORDs. Faithful port: returns `n` bits
// positioned in the TOP of a u32; callers shift right as needed. State and
// operation order match the reference exactly (do not "clean up").

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    curr_dword: u32,
    bit_pos: u32, // curr_dword_bit_pos
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
                return out;
            }
            let entry_index = table.lookup[(info_bytes & table.mask) as usize];
            code_entry = &table.entries[entry_index as usize];

            num_bits += code_entry.u8_0 as u32;
            info_bytes >>= code_entry.u8_0;

            if code_entry.cmd_type != CMD_COPY_PREV_BYTES {
                break;
            }

            let next_bits = br.next_bits(num_bits);
            info_bytes |= next_bits;

            let amt = (code_entry.u16_1 as usize) + ((info_bytes & ((1u32 << code_entry.u8_1) - 1)) as usize);
            info_bytes >>= code_entry.u8_1;

            let back = (code_entry.u16_3 as u32) + ((info_bytes & ((1u32 << code_entry.u8_2) - 1)) << unknown_11);
            info_bytes >>= code_entry.u8_2;

            let amt = amt.min(out_size - write_address);
            let src = write_address - back as usize;
            out.copy_within(src..src + amt, write_address);
            write_address += amt;

            num_bits = code_entry.u8_2 as u32 + code_entry.u8_1 as u32;
        }

        // Outer: COPY_BYTES (cmd 2) or COPY_BYTE (cmd 1), using the breaking entry.
        if code_entry.cmd_type == CMD_COPY_BYTES {
            let next_bits = br.next_bits(num_bits);
            info_bytes |= next_bits;

            let amt = (code_entry.u16_1 as usize) + ((info_bytes & ((1u32 << code_entry.u8_1) - 1)) as usize);
            info_bytes >>= code_entry.u8_1;
            num_bits = code_entry.u8_1 as u32;

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
    let unknown = u16(data, 2) as usize;
    if unknown > 64 {
        return Err(format!("invalid unknown (<=64, got {})", unknown));
    }
    if &data[4..12] != b"CPRNAV_2" {
        return Err(format!(
            "bad signature {:?} (expected \"CPRNAV_2\")",
            String::from_utf8_lossy(&data[4..12])
        ));
    }

    let block_size = unknown * 0x400;
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

pub fn decompress(data: &[u8]) -> Result<Vec<u8>, String> {
    let hdr = parse_header(data)?;
    let tables = code_tables();
    let mut out = vec![0u8; hdr.unpacked_size];
    let mut pos = 0usize;

    for (start, end) in &hdr.blocks {
        if *end > data.len() {
            return Err(format!("block [{:#x},{:#x}] exceeds file size {}", start, end, data.len()));
        }
        let block = &data[*start..*end];
        let unpacked = unpack_block(block, &tables, hdr.block_size);
        out[pos..pos + unpacked.len()].copy_from_slice(&unpacked);
        pos += unpacked.len();
        // Pad to the next block boundary (matching the reference's zero fill).
        while pos % hdr.block_size != 0 && pos < hdr.unpacked_size {
            pos += 1;
        }
    }

    Ok(out)
}

// --- CLI ------------------------------------------------------------------

fn is_cprnav(data: &[u8]) -> bool {
    data.len() >= 12
        && u16::from_le_bytes([data[0], data[1]]) == 5
        && &data[4..12] == b"CPRNAV_2"
}

fn default_out(input: &Path) -> PathBuf {
    let mut full = input.file_name().map(|s| s.to_os_string()).unwrap_or_default();
    full.push(".BIN");
    match input.parent() {
        Some(p) => p.join(full),
        None => PathBuf::from(full),
    }
}

fn decompress_file(input: &Path, out: &Path) -> Result<(usize, usize), String> {
    let data = fs::read(input).map_err(|e| format!("read {}: {}", input.display(), e))?;
    if !is_cprnav(&data) {
        return Err(format!("{} is not a CPRNAV_2 file", input.display()));
    }
    let unpacked = decompress(&data)?;
    fs::write(out, &unpacked).map_err(|e| format!("write {}: {}", out.display(), e))?;
    Ok((data.len(), unpacked.len()))
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.is_empty() || args[0].eq_ignore_ascii_case("-h") || args[0] == "--help" {
        eprintln!(
            "Usage:\n  cprnav_decompress_rs <file> [out]\n  cprnav_decompress_rs <dir>  [outdir]"
        );
        exit(if args.is_empty() { 1 } else { 0 });
    }

    let target = Path::new(&args[0]);
    let mut ok = 0usize;
    let mut failed: Vec<String> = Vec::new();

    if target.is_dir() {
        let outdir = args.get(1).map(Path::new).unwrap_or(target);
        fs::create_dir_all(outdir).ok();
        let entries = match fs::read_dir(target) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("read dir {}: {}", target.display(), e);
                exit(1);
            }
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if !p.is_file() {
                continue;
            }
            let outname = default_out(&p).file_name().unwrap_or_default().to_os_string();
            match decompress_file(&p, &outdir.join(&outname)) {
                Ok((a, b)) => {
                    println!("{:<40} {:>10} -> {:>10}", p.display(), a, b);
                    ok += 1;
                }
                Err(msg) if msg.contains("not a CPRNAV_2 file") => {}
                Err(msg) => failed.push(format!("{}: {}", p.display(), msg)),
            }
        }
    } else {
        let out = match args.get(1) {
            Some(o) => PathBuf::from(o),
            None => default_out(target),
        };
        match decompress_file(target, &out) {
            Ok((a, b)) => {
                println!("{:<40} {:>10} -> {:>10}", target.display(), a, b);
                ok += 1;
            }
            Err(msg) => {
                eprintln!("error: {}", msg);
                exit(1);
            }
        }
    }

    if !failed.is_empty() {
        for f in &failed {
            eprintln!("FAILED {}", f);
        }
        exit(2);
    }
    if ok == 0 && target.is_dir() {
        eprintln!("no CPRNAV_2 files found in {}", target.display());
        exit(3);
    }
}
