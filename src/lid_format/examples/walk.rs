// Walk the FLI city trie of a name-list file with one or more query strings and print each step.
// usage: walk <file.DAT> <query> [query2 ...]
use lid_format::device_sim::{check_name_list_header, dump_block_edges, walk_city_query};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: walk <file.DAT> <query> [query2 ...]");
        std::process::exit(2);
    }
    let file = &args[1];
    let b = std::fs::read(file).expect("read file");
    let h = match check_name_list_header("w", &b) {
        Ok(h) => h,
        Err(g) => {
            eprintln!("LoadHeader FAIL: {}", g.0);
            std::process::exit(1);
        }
    };
    println!("file={} nb={} elem={}", file, h.nb, h.elem);
    for q in args.iter().skip(2) {
        if q == "DUMP" {
            println!("\n=== DUMP block0 edges (node -> [(label, child, claim)]) ===");
            match dump_block_edges(&b, &h, 0) {
                Ok(edges) => {
                    for (n, es) in edges.iter().take(40) {
                        let s: Vec<String> = es
                            .iter()
                            .map(|(l, c, cl)| format!("'{}'->{}(c{})", l, c, cl))
                            .collect();
                        println!("  node {n}: {}", s.join(" "));
                    }
                }
                Err(g) => println!("   ERR: {}", g.0),
            }
            continue;
        }
        println!("\n=== walk {:?} ===", q);
        match walk_city_query(&b, &h, 0, q) {
            Ok(t) => {
                for s in &t.steps {
                    println!("   {}", s);
                }
                let tag = if t.success { "SUCCESS" } else { "FAIL" };
                println!("   => {} : {}", tag, t.message);
                if let Some(e) = t.element_id {
                    println!("   element_id = {}", e);
                }
            }
            Err(g) => println!("   ERR: {}", g.0),
        }
    }
}
