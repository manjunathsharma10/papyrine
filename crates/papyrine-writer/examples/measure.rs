//! Timings for the three outputs on one file: `measure <file.pdf> <workdir> [password]`.

use std::path::Path;
use std::time::Instant;

use papyrine_cos::{Document, ObjectStreams, OpenOptions, Secret, WriteOptions};
use papyrine_writer::*;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (src, work) = (Path::new(&args[1]), Path::new(&args[2]));
    let pw = args.get(3).map(|p| Secret::from(p.as_str()));
    std::fs::create_dir_all(work).unwrap();
    let file = work.join("doc.pdf");
    let t = Instant::now();
    std::fs::copy(src, &file).unwrap();
    println!("copy source            {:9.1} ms", ms(t));
    let size = std::fs::metadata(&file).unwrap().len();

    let opts = OpenOptions {
        password: pw.clone(),
        ..OpenOptions::default()
    };
    let t = Instant::now();
    let doc = Document::open_path(&file, &opts).unwrap();
    println!(
        "open (qpdf)            {:9.1} ms   {} objects, {} MB",
        ms(t),
        doc.object_count().unwrap(),
        size >> 20
    );

    // Dirty set: the first page, the Info dictionary and a 50-object spread.
    let ids = doc.object_ids().unwrap();
    let mut dirty = vec![doc.page(0).unwrap().id().unwrap()];
    for id in ids.iter().step_by((ids.len() / 50).max(1)).take(50) {
        let o = doc.object(*id).unwrap();
        if o.kind().unwrap() == papyrine_cos::ObjectKind::Dictionary {
            o.dict_set("PapyrineMeasure", &doc.new_int(1)).unwrap();
            dirty.push(*id);
        }
    }
    dirty.sort();
    dirty.dedup();

    let t = Instant::now();
    let signed = has_signatures(&doc).unwrap();
    println!("signature scan (objs)  {:9.1} ms   signed: {signed}", ms(t));

    let t = Instant::now();
    let chain = ChainState::scan(&std::fs::File::open(&file).unwrap()).unwrap();
    println!(
        "scan chain tail        {:9.2} ms   {:?}, {} sections",
        ms(t),
        chain.kind,
        chain.sections
    );

    let t = Instant::now();
    let s = write_section(
        &doc,
        &chain,
        &SectionRequest {
            dirty: dirty.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    println!(
        "section ({:3} objects)  {:9.2} ms   {} bytes",
        dirty.len(),
        ms(t),
        s.bytes.len()
    );

    let one = vec![dirty[0]];
    let t = Instant::now();
    let s1 = write_section(
        &doc,
        &chain,
        &SectionRequest {
            dirty: one,
            ..Default::default()
        },
    )
    .unwrap();
    println!(
        "section (1 object)     {:9.2} ms   {} bytes",
        ms(t),
        s1.bytes.len()
    );

    let t = Instant::now();
    let r = save_incremental(
        &doc,
        &file,
        &file,
        &SectionRequest {
            dirty: dirty.clone(),
            ..Default::default()
        },
        SaveOptions {
            password: pw.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    println!(
        "save incremental       {:9.1} ms   +{} bytes (copy + validate + rename)",
        ms(t),
        r.appended
    );

    let t = Instant::now();
    let r = save_incremental(
        &doc,
        &file,
        &work.join("skip.pdf"),
        &SectionRequest {
            dirty: dirty.clone(),
            ..Default::default()
        },
        SaveOptions {
            password: pw.clone(),
            skip_prefix_check: true,
            ..Default::default()
        },
    )
    .unwrap();
    println!(
        "  ... no prefix check  {:9.1} ms   +{} bytes",
        ms(t),
        r.appended
    );

    let t = Instant::now();
    let mut sink = std::io::sink();
    let rep = write_full(&doc, &mut sink, &FullWriteOptions::default(), &|| false).unwrap();
    println!(
        "full write (to sink)   {:9.1} ms   {} objects, {} MB",
        ms(t),
        rep.objects,
        rep.bytes >> 20
    );

    let t = Instant::now();
    let r = save_optimized(
        &doc,
        &work.join("optimized.pdf"),
        &WriteOptions {
            object_streams: ObjectStreams::Generate,
            ..Default::default()
        },
        SaveOptions {
            password: pw,
            ..Default::default()
        },
    )
    .unwrap();
    println!(
        "save optimized (qpdf)  {:9.1} ms   {} MB",
        ms(t),
        r.bytes_written >> 20
    );
}
