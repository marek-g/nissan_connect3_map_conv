use std::io::Read;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut v = Vec::new();
    std::fs::File::open(&args[1]).unwrap().read_to_end(&mut v).unwrap();
    if args.len() > 2 && args[2] == "-s" {
        print!("{}", lid_format::debug_find(&v, &args[3]));
    } else {
        let lo: usize = args.get(2).map_or(0, |s| s.parse().unwrap());
        let hi: usize = args.get(3).map_or(60, |s| s.parse().unwrap());
        print!("{}", lid_format::debug_trie(&v, lo, hi));
    }
}
