// Report the block-link (DAG) graph of a name-list file and DFS reachability from block 0.
use lid_format::read_links;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let b = std::fs::read(&args[1]).unwrap();
    let links = read_links(&b).unwrap();
    println!("blocks={}", links.len());
    for (bi, m) in links.iter().enumerate() {
        let mut tg: Vec<usize> = m.values().map(|v| v.0).collect();
        tg.sort_unstable();
        tg.dedup();
        println!("block {bi}: links={} targets={:?}", m.len(), tg);
    }
    let mut reach = vec![false; links.len()];
    let mut q = vec![0usize];
    reach[0] = true;
    while let Some(bi) = q.pop() {
        for &(tb, _) in links[bi].values() {
            if tb < links.len() && !reach[tb] {
                reach[tb] = true;
                q.push(tb);
            }
        }
    }
    let bad: Vec<usize> = (0..links.len()).filter(|&i| !reach[i]).collect();
    println!("UNREACHABLE from block 0: {:?}", bad);
}
