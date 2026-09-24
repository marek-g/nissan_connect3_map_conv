// Dump descriptor table + raw link-column bytes of one block. Usage: blockdump <file> <block>
use lid_format::read_link_infos;

fn u16at(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes(b[p..p + 2].try_into().unwrap())
}
fn u32at(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes(b[p..p + 4].try_into().unwrap())
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let b = std::fs::read(&a[1]).unwrap();
    let bi: usize = a[2].parse().unwrap();
    let links = read_link_infos(&b).unwrap();
    println!(
        "links of block {bi}: {:?} nodes={} leaves={}",
        links[bi].links, links[bi].node_count, links[bi].leaf_count
    );
    let hdr = u32at(&b, 0x10) as usize;
    let nblk = u16at(&b, hdr + 4) as usize;
    let sec1 = u32at(&b, hdr + 24 + 5 + 1) as usize;
    let bs = u32at(&b, sec1 + 4 * bi) as usize;
    let be = if bi + 1 < nblk {
        u32at(&b, sec1 + 4 * (bi + 1)) as usize
    } else {
        b.len()
    };
    let node_count = u16at(&b, bs);
    let nbel = u16at(&b, bs + 4);
    let num_desc = u16at(&b, bs + 6) as usize;
    println!(
        "block {bi}: bs={bs:#x} len={:#x} nodes={node_count} elems={nbel} descs={num_desc}",
        be - bs
    );
    for i in 0..num_desc {
        let p = bs + 8 + i * 12;
        let k = u16at(&b, p);
        println!(
            "  row {i}: kind={:#06x} flags={:#06x} code={:#04x} off={:#x} param={:#x}",
            k & 0xfff,
            k & 0xf000,
            u16at(&b, p + 2),
            u32at(&b, p + 4),
            u32at(&b, p + 8)
        );
    }
}
