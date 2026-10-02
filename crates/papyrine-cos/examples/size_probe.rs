// Opens a PDF, rewrites it with object streams, prints the sizes. Used to measure the binary
// size contributed by the static qpdf build: compare with `size_baseline`.
use papyrine_cos::{Document, ObjectStreams, OpenOptions, WriteOptions};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: size_probe <file.pdf>");
    let data = std::fs::read(&path).expect("read");
    let doc = Document::open_bytes(data, &OpenOptions::default()).expect("open");
    let out = doc
        .write(&WriteOptions {
            object_streams: ObjectStreams::Generate,
            ..WriteOptions::default()
        })
        .expect("write");
    println!(
        "{} pages, {} repairs, {} bytes out",
        doc.page_count().unwrap_or(0),
        doc.repair_log().len(),
        out.bytes().len()
    );
}
