// Append one synthetic entry to a stock name-list as a loader-probe experiment.
// Usage: zzcity <stock.DAT> <out.DAT> [city]
// With "city" the probe is GRAFTED through a stock block-link (last-block leaf X becomes the
// last-block leaf gains a block-link to a new block spelling "TEST"; probe name = lost + "TEST").
// Without it the classic street/gazetteer append layout is used.
use lid_format::NameEntry;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (src, dst) = (&args[0], &args[1]);
    let city_mode = args.get(2).is_some_and(|m| m == "city");
    let stock = std::fs::read(src).unwrap();
    if city_mode {
        // Two sequential grafts, both into the (then-)LAST stock block (44): each graft turns one
        // host leaf into a block-link sink and reserves a NEW trailing block index. Graft ORDER
        // pairs with merge chunk (block index) order: host#1 -> block nb, host#2 -> block nb+1.
        // ASCII host (ZYRZYN) + ASCII suffix proves typeahead works without Polish chars.
        let (g1, r1) = lid_format::graft_city_probe(&stock, "AATEST", "ZYRZYN", 0).unwrap();
        println!(
            "graft1: host block {} node {} lost element '{}' -> probe '{}'",
            r1.host_block, r1.host_node, r1.lost_name, r1.probe_raw
        );
        let (g2, r2) = lid_format::graft_city_probe(&g1, "TEST", "ZŁOTY POTOK", 1).unwrap();
        println!(
            "graft2: host block {} node {} lost element '{}' -> probe '{}'",
            r2.host_block, r2.host_node, r2.lost_name, r2.probe_raw
        );
        let es = [
            NameEntry {
                label: "AATEST".to_string(),
                x_pau: 255012,
                y_pau: 49638,
                city: Some((0, 0)),
                belonging: None,
            },
            NameEntry {
                label: "TEST".to_string(),
                x_pau: 255013,
                y_pau: 49639,
                city: Some((1, 0)), // distinct anchor -> distinct chunk -> distinct new block
                belonging: None,
            },
        ];
        let (out, base) = lid_format::merge_city_list(&g2, &es).unwrap();
        std::fs::write(dst, &out).unwrap();
        println!("base elem {base}, out {} bytes", out.len());
        let links = lid_format::read_links(&out).unwrap();
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
        println!("reachability: {} blocks, unreachable {bad:?}", links.len());
        println!("host block links (node -> target): {:?}", links[44]);
        let nl = lid_format::read(&out).unwrap();
        for el in &nl.elements {
            if el.sort_name.contains("QTEST") || el.sort_name.contains("TEST") {
                println!(
                    "probe element: block {} node '{}' name '{}'",
                    el.block,
                    el.node,
                    el.sort_name.replace('\t', "\\t")
                );
            }
        }
        println!("elem count header check: {}", nl.element_count);
    } else {
        let e = NameEntry {
            label: "AATEST".to_string(),
            x_pau: 0,
            y_pau: 0,
            city: None,
            belonging: None,
        };
        let (out, base) = lid_format::merge_name_list(&stock, &[e]).unwrap();
        std::fs::write(dst, &out).unwrap();
        println!("base elem {base}, out {} bytes", out.len());
    }
}
