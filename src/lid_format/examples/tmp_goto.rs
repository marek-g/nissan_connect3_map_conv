// Faithful bGoToElemet walk (empty filter -> first file-order edge per node, exact [id,id] end).
use std::collections::HashMap;

use lid_format::device_sim::{check_block_load, check_name_list_header};
use lid_format::read_links;

struct B {
    degs: Vec<u32>,
    claims: Vec<u32>,
    cs: Vec<usize>,
    link: Vec<bool>,
    links: HashMap<usize, (usize, usize)>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = &args[1];
    let b = std::fs::read(path).expect("read");
    let h = check_name_list_header("x", &b).unwrap();
    println!("elem={} blocks={}", h.elem, h.nb);
    let links_all = read_links(&b).unwrap();
    let mut bs: Vec<B> = Vec::new();
    for bi in 0..h.nb as usize {
        let bl = check_block_load("x", &b, &h, bi).unwrap();
        let nc = bl.degs.len();
        // CalculateFirstEdgeIndex replica (pure DFS, overwrite per visit)
        let mut cs = vec![0usize; nc];
        let mut root = 0usize;
        let mut base = 1usize;
        while root < nc {
            let mut stack = vec![root];
            let mut visits = 0usize;
            let mut ec = 0usize;
            while let Some(n) = stack.pop() {
                cs[n] = base + ec;
                ec += bl.degs[n] as usize;
                visits += 1;
                let c0 = cs[n];
                for i in (0..bl.degs[n] as usize).rev() {
                    let c = c0 + i;
                    if c < nc {
                        stack.push(c);
                    }
                }
            }
            root += visits;
            base += 1;
        }
        bs.push(B {
            degs: bl.degs,
            claims: bl.claims,
            cs,
            link: bl.link_bits.clone(),
            links: links_all.get(bi).cloned().unwrap_or_default(),
        });
    }
    let root_cl: usize = bs[0].claims[..bs[0].degs[0] as usize]
        .iter()
        .map(|&x| x as usize)
        .sum();
    println!(
        "block0: nodes={} root_deg={} domain={}",
        bs[0].degs.len(),
        bs[0].degs[0],
        root_cl
    );

    if args.get(2).map(|s| s.as_str()) == Some("scan") {
        let mut ok = Vec::new();
        for id in 0..h.elem {
            let (r, _) = goto(&bs, id);
            if r {
                ok.push(id);
            }
        }
        println!("resolved {}/{} ids", ok.len(), h.elem);
        println!("first 60: {:?}", &ok[..ok.len().min(60)]);
        return;
    }
    let ids: Vec<u32> = args
        .iter()
        .skip(2)
        .map(|s| s.parse().expect("id"))
        .collect();
    let ids = if ids.is_empty() {
        vec![
            194288, // BYDGOSZCZ
            199864, // GDANSK
            208837, // KRAKOW
            208838, // KRAKOW (O)
            211986, // LODZ
            212340, // LUBLIN
            222142, // POZNAN
            222147, // POZNAN (N)
            223516, // RADOM
            231070, // SZCZECIN
            233759, // WARSZAWA
            236833, // WROCLAW
        ]
    } else {
        ids
    };
    for id in ids {
        let (ok, path) = goto(&bs, id);
        println!(
            "id {:7}: {} {}",
            id,
            if ok { "OK  " } else { "FAIL" },
            path
        );
    }
}

fn goto(bs: &[B], id: u32) -> (bool, String) {
    let mut blk = 0usize;
    let mut node = 0usize;
    let mut start = 0usize; // interval start of current node's edges
    let mut path = String::from("r0");
    for _ in 0..400 {
        let b = &bs[blk];
        let d = *b.degs.get(node).unwrap_or(&0) as usize;
        if d == 0 {
            return (false, format!("{path} [no edges]"));
        }
        let e = b.cs[node] - 1; // fe[n]: first edge index (target(e) = e+1 = cs[n])
        if e >= b.claims.len() {
            return (false, format!("{path} [edge oob {}]", e));
        }
        let c = b.claims[e] as usize;
        if start == id as usize && c == 1 {
            let child = e + 1;
            let is_elem = child < b.degs.len() && b.degs[child] == 0;
            return (
                true,
                format!(
                    "{path}/e{}(c=1) leaf={}",
                    e,
                    if is_elem { "Y" } else { "N?" }
                ),
            );
        }
        let child = e + 1;
        if child >= b.degs.len() {
            return (false, format!("{path} [child oob {}]", child));
        }
        path.push_str(&format!("/n{}:e{}(c{})", node, e, c));
        // S_edge = start + 0 (first edge) -> unchanged
        if b.link.get(child).copied().unwrap_or(false) {
            match b.links.get(&child) {
                Some(&(tb, tn)) => {
                    path.push_str(&format!("=>b{tb}:n{tn}"));
                    blk = tb;
                    node = tn;
                }
                None => return (false, format!("{path} [no link target]")),
            }
        } else {
            node = child;
        }
    }
    (false, format!("{path} [depth limit]"))
}
