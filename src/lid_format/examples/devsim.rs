// Simulate device enumeration view: per-block element counts + link table.
use lid_format::*;
fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let b = std::fs::read(&args[1]).map_err(|e| e.to_string())?;
    let nl = read(&b)?;
    let links = read_links(&b).map_err(|e| e.to_string())?;
    println!("elements: {}", nl.elements.len());
    let mut counts = vec![0usize; links.len()];
    for el in &nl.elements {
        if el.block < counts.len() {
            counts[el.block] += 1;
        }
    }
    for (i, (c, l)) in counts.iter().zip(&links).enumerate() {
        println!("block {i}: elems={c} links={}", l.len());
    }
    Ok(())
}
