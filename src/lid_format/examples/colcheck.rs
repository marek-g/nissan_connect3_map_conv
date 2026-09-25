use lid_format::col406_audit;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    col406_audit(&std::fs::read(&a[1]).unwrap());
}
