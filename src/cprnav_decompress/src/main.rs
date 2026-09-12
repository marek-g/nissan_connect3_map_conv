// CPRNAV_2 decompressor — command-line front-end.
//
// All codec logic lives in the `cprnav` library (shared with cprnav_compress);
// this binary only walks the given file/dir and does I/O.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::exit;

use cprnav::{decompress, is_cprnav};

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
            "Usage:\n  cprnav_decompress <file> [out]\n  cprnav_decompress <dir>  [outdir]"
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
