//! `lopdf-probe <file>`: open the file with lopdf, resolve every page object, print one
//! `RESULT` line. Run one process per file (compare.py) so peak RSS, crashes and hangs are
//! attributable. Panics are not caught: exit code 101 is reported by the runner.
//!
//! RESULT\t<status>\t<pages>\t<objects>\t<open_ms>\t<first_pages_ms>\t<detail>
use std::time::Instant;

fn main() {
    let path = std::env::args().nth(1).expect("usage: lopdf-probe <file>");
    let t0 = Instant::now();
    let doc = match lopdf::Document::load(&path) {
        Ok(d) => d,
        Err(e) => {
            let msg = e.to_string().replace(['\t', '\n'], " ");
            println!("RESULT\terr\t-\t-\t{}\t0\t{msg}", t0.elapsed().as_millis());
            return;
        }
    };
    let open_ms = t0.elapsed().as_millis();
    // Time to first object: page count (walks the page tree).
    let t1 = Instant::now();
    let pages = doc.get_pages();
    let first_ms = t1.elapsed().as_millis();
    let mut bad = 0usize;
    for id in pages.values() {
        if doc.get_object(*id).is_err() {
            bad += 1;
        }
    }
    let status = if pages.is_empty() {
        "ok-no-pages"
    } else {
        "ok"
    };
    println!(
        "RESULT\t{status}\t{}\t{}\t{open_ms}\t{first_ms}\tunresolved_pages={bad}",
        pages.len(),
        doc.objects.len()
    );
}
