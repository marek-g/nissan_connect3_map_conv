// CPRNAV_2 compressor — command-line front-end.
//
// All codec logic lives in the `cprnav` library (shared with cprnav_decompress);
// this binary only parses arguments, guards against nesting, and does file I/O.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;

use cprnav::{compress, is_cprnav, CompressOptions};

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
            "Usage:\n  cprnav_compress <file> [out] [--level N] [--block-kib K] [--table T]\n\n\
             Compress a file into CPRNAV_2 (inverse of cprnav_decompress).\n\
             Defaults: level=9 (best), block-kib=16 (0x4000-byte blocks), auto code-table.\n\
             --level N      1-9. 1-4 fast greedy; 5-9 optimal-parsing (best) + bigger window\n\
             --no-lz        disable LZ77 back refs (literal only)\n\
             --block-kib K  block size in KiB: 16 -> 0x4000 bytes, 64 -> 0x10000 bytes\n\
             --table T      force code table 0..3 (default: auto-pick best per block)\n\
             --force        compress even if the input already looks CPRNAV_2 (nested)\n\n\
             PARALLEL: the cprnav library uses rayon to compress blocks and select tables concurrently."
        );
        exit(if args.is_empty() { 1 } else { 0 });
    }

    let mut opts = CompressOptions::default();
    let mut force = false;
    let mut files: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lz" => opts.literal_only = false,
            "--no-lz" => opts.literal_only = true,
            "--level" => {
                i += 1;
                opts.level = args[i].parse().expect("bad --level");
            }
            "--block-kib" => {
                i += 1;
                opts.block_size_kib = args[i].parse().expect("bad --block-kib");
            }
            "--table" => {
                i += 1;
                opts.table = Some(args[i].parse().expect("bad --table"));
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
    if is_cprnav(&data) {
        eprintln!(
            "error: {} already looks like a CPRNAV_2 archive (version=5, magic at +4). \
             Decompress it first (cprnav_decompress) before compressing; the runtime cannot read nested files. \
             Use --force to override.",
            input.display()
        );
        if !force {
            exit(2);
        }
    }
    let packed = compress(&data, opts);
    if let Err(e) = fs::write(&out, &packed) {
        eprintln!("write {}: {}", out.display(), e);
        exit(1);
    }
    println!("{:<40} {:>10} -> {:>10}  ({})", input.display(), data.len(), packed.len(), out.display());
}
