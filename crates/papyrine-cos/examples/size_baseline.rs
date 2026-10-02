// Baseline for measuring what papyrine-cos + qpdf adds to a release binary (see size_probe).
fn main() {
    let path = std::env::args().nth(1).unwrap_or_default();
    let data = std::fs::read(&path).unwrap_or_default();
    println!("{}", data.len());
}
