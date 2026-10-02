use papyrine_render::testgen::*;
use papyrine_render::*;
use std::process::Command;

fn lib() -> std::sync::Arc<Library> {
    Library::global().expect("libpdfium: run tools/fetch-pdfium")
}

fn open(bytes: Vec<u8>, sections: &[Vec<u8>]) -> Document {
    let secs: Vec<Bytes> = sections.iter().cloned().map(bytes_from_vec).collect();
    Document::open(&lib(), bytes_from_vec(bytes), &secs, None).expect("open")
}

fn tile(d: &mut Document, page: usize, bucket: i32, col: u32, row: u32) -> Tile {
    d.render_tile(page, bucket, TileCoord { col, row }, &mut never_cancel)
        .unwrap()
}

fn hello() -> Built {
    build(&[PageSpec::letter(hello_content())])
}

#[test]
fn page_info_and_rotation() {
    let b = build(&[
        PageSpec::letter(hello_content()),
        PageSpec {
            width: 400.0,
            height: 600.0,
            rotate: 90,
            content: hello_content(),
        },
    ]);
    let mut d = open(b.bytes, &[]);
    assert_eq!(d.page_count(), 2);
    assert_eq!(d.page_size(0).unwrap(), (612.0, 792.0));
    assert_eq!(d.page_size(1).unwrap(), (600.0, 400.0));
    assert_eq!(d.page_rotation(0).unwrap(), 0);
    assert_eq!(d.page_rotation(1).unwrap(), 90);
    assert_eq!(d.page_size(2), Err(Error::PageOutOfRange(2)));
    assert_eq!(d.page_sizes().unwrap().len(), 2);
}

#[test]
fn tile_pixels_and_edges() {
    let mut d = open(hello().bytes, &[]);
    // 612x792 at bucket 0: 2x2 tiles; black square is device x 100..300, y 492..692.
    let t00 = tile(&mut d, 0, 0, 0, 0);
    assert_eq!((t00.width, t00.height), (512, 512));
    assert_eq!(t00.pixel(10, 10), [255, 255, 255, 255]);
    assert_eq!(t00.pixel(200, 500), [0, 0, 0, 255]);
    assert_eq!(t00.pixel(200, 490), [255, 255, 255, 255]);
    // The square straddles the tile boundary at y=512.
    let t01 = tile(&mut d, 0, 0, 0, 1);
    assert_eq!((t01.width, t01.height), (512, 280));
    assert_eq!(t01.pixel(200, 0), [0, 0, 0, 255]);
    assert_eq!(t01.pixel(200, 179), [0, 0, 0, 255]); // y = 691
    assert_eq!(t01.pixel(200, 181), [255, 255, 255, 255]); // y = 693
    let t11 = tile(&mut d, 0, 0, 1, 1);
    assert_eq!((t11.width, t11.height), (100, 280));
    assert!(t11.rgba.chunks(4).all(|p| p == [255, 255, 255, 255]));
    assert!(
        d.render_tile(0, 0, TileCoord { col: 2, row: 0 }, &mut never_cancel)
            .is_err()
    );
}

#[test]
fn zoom_bucket_scales_content() {
    let mut d = open(hello().bytes, &[]);
    // bucket 2 => scale 2: square at x 200..600, y 984..1384; tile (0,2) covers y 1024..1536.
    let t = tile(&mut d, 0, 2, 0, 2);
    assert_eq!(t.pixel(300, 100), [0, 0, 0, 255]);
    assert_eq!(t.pixel(300, 400), [255, 255, 255, 255]); // y = 1424
    assert_eq!(t.pixel(650, 100), [255, 255, 255, 255]); // x = 650
    // bucket -2 => scale 0.5: whole page is one 306x396 tile; square x 50..150, y 246..346.
    let t = tile(&mut d, 0, -2, 0, 0);
    assert_eq!((t.width, t.height), (306, 396));
    assert_eq!(t.pixel(100, 300), [0, 0, 0, 255]);
}

#[test]
fn preview_fits_max_edge() {
    let mut d = open(hello().bytes, &[]);
    let p = d.render_preview(0, 256, &mut never_cancel).unwrap();
    assert_eq!((p.width, p.height), (198, 256));
    assert_eq!(p.pixel(60, 190), [0, 0, 0, 255]);
}

#[test]
fn incremental_section_changes_render() {
    let b = hello();
    let mut chain = Chain::new(&b);
    let spec = PageSpec::letter(red_content());
    let sec = chain.section(&page_objects(&b, 0, &spec));
    // Base only: black. Base + section: red.
    let mut base_only = open(b.bytes.clone(), &[]);
    assert_eq!(
        tile(&mut base_only, 0, 0, 0, 0).pixel(200, 500),
        [0, 0, 0, 255]
    );
    let mut patched = open(b.bytes.clone(), std::slice::from_ref(&sec));
    assert_eq!(patched.byte_len() as usize, b.bytes.len() + sec.len());
    assert_eq!(
        tile(&mut patched, 0, 0, 0, 0).pixel(200, 500),
        [255, 0, 0, 255]
    );
    // Re-open in place: base-only document picks up the section.
    base_only.reopen(&[bytes_from_vec(sec.clone())]).unwrap();
    assert_eq!(
        tile(&mut base_only, 0, 0, 0, 0).pixel(200, 500),
        [255, 0, 0, 255]
    );
    // ... and goes back when the section is dropped.
    base_only.reopen(&[]).unwrap();
    assert_eq!(
        tile(&mut base_only, 0, 0, 0, 0).pixel(200, 500),
        [0, 0, 0, 255]
    );
}

#[test]
fn chained_sections_latest_wins() {
    let b = hello();
    let mut chain = Chain::new(&b);
    let green = PageSpec::letter(b"0 1 0 rg 100 100 200 200 re f".to_vec());
    let s1 = chain.section(&page_objects(&b, 0, &PageSpec::letter(red_content())));
    let filler = chain.alloc_id();
    let mut rng = Rng(7);
    let s2 = chain.section(&[
        (filler, filler_body(5000, &mut rng)),
        (b.content_ids[0], stream_body("", &green.content)),
    ]);
    let mut d = open(b.bytes, &[s1, s2]);
    assert_eq!(tile(&mut d, 0, 0, 0, 0).pixel(200, 500), [0, 255, 0, 255]);
}

#[test]
fn reopen_failure_keeps_old_document() {
    let b = hello();
    let mut d = open(b.bytes, &[]);
    let junk = bytes_from_vec(b"%PDF-1.7 not really".to_vec());
    // Garbage appended after the xref is tolerated or rejected, but never corrupts the old doc.
    let _ = d.reopen(&[junk]);
    assert!(d.page_count() >= 1);
    assert_eq!(tile(&mut d, 0, 0, 0, 0).pixel(10, 10), [255, 255, 255, 255]);
}

#[test]
fn text_extraction_boxes_and_hit_test() {
    let mut d = open(hello().bytes, &[]);
    let t = d.page_text(0).unwrap();
    assert!(t.text.contains("Hello Papyrine"), "{:?}", t.text);
    assert_eq!(t.text.chars().count(), t.chars.len());
    let h = t.text.find('H').unwrap();
    let c = t.chars[h];
    assert_eq!(c.ch, 'H');
    assert!((c.left - 72.0).abs() < 2.0, "left {}", c.left);
    assert!(c.bottom > 690.0 && c.bottom < 705.0, "bottom {}", c.bottom);
    assert!(c.top - c.bottom > 10.0 && c.right > c.left);
    assert!(c.loose_top >= c.top - 0.01);
    // Hit test at the centre of 'H', and on empty space.
    let (cx, cy) = ((c.left + c.right) / 2.0, (c.bottom + c.top) / 2.0);
    assert_eq!(d.char_at(0, cx, cy, 1.0, 1.0).unwrap(), Some(h));
    assert_eq!(d.char_at(0, 500.0, 100.0, 1.0, 1.0).unwrap(), None);
}

#[test]
fn page_cache_is_capped() {
    let pages: Vec<_> = (0..20).map(|_| PageSpec::letter(hello_content())).collect();
    let mut d = open(build(&pages).bytes, &[]);
    for i in 0..20 {
        tile(&mut d, i, -4, 0, 0);
        assert!(d.cached_pages().len() <= PAGE_CACHE_CAP);
    }
    assert_eq!(d.cached_pages().len(), PAGE_CACHE_CAP);
    assert_eq!(d.cached_pages()[0], 19); // MRU first
    d.page_text(11).unwrap();
    assert_eq!(d.cached_pages()[0], 11);
    d.reopen(&[]).unwrap();
    assert!(d.cached_pages().is_empty());
}

#[test]
fn cancel_aborts_heavy_render() {
    let mut rng = Rng(42);
    let content = busy_content(1, 40, 20_000, &mut rng);
    let mut d = open(build(&[PageSpec::letter(content)]).bytes, &[]);
    let mut polls = 0;
    let r = d.render_tile(0, 0, TileCoord { col: 0, row: 0 }, &mut || {
        polls += 1;
        true
    });
    assert_eq!(r, Err(Error::Cancelled));
    assert!(polls >= 1);
    // The document is still usable and a later render completes.
    assert!(
        d.render_tile(0, 0, TileCoord { col: 0, row: 0 }, &mut never_cancel)
            .is_ok()
    );
}

#[test]
fn garbage_is_rejected() {
    let r = Document::open(
        &lib(),
        bytes_from_vec(b"hello, not a pdf".to_vec()),
        &[],
        None,
    );
    assert!(matches!(r, Err(Error::Format)));
}

#[test]
fn mmap_open() {
    let dir = std::env::temp_dir().join(format!("papyrine-render-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("m.bin");
    std::fs::write(&path, hello().bytes).unwrap();
    let map = open_mmap(&path).unwrap();
    let mut d = Document::open(&lib(), map, &[], None).unwrap();
    assert_eq!(tile(&mut d, 0, 0, 0, 0).pixel(200, 500), [0, 0, 0, 255]);
    drop(d);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn password_flow_via_qpdf() {
    let Ok(v) = Command::new("qpdf").arg("--version").output() else {
        eprintln!("qpdf not installed; skipping");
        return;
    };
    assert!(v.status.success());
    let dir = std::env::temp_dir().join(format!("papyrine-render-pw-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (src, dst) = (dir.join("in.bin"), dir.join("enc.bin"));
    std::fs::write(&src, hello().bytes).unwrap();
    let st = Command::new("qpdf")
        .args(["--encrypt", "user-pw", "owner-pw", "256", "--"])
        .arg(&src)
        .arg(&dst)
        .status()
        .unwrap();
    assert!(st.success());
    let bytes = std::fs::read(&dst).unwrap();
    let open = |pw: Option<&str>| Document::open(&lib(), bytes_from_vec(bytes.clone()), &[], pw);
    assert!(matches!(open(None), Err(Error::PasswordRequired)));
    assert!(matches!(open(Some("nope")), Err(Error::PasswordIncorrect)));
    let mut d = open(Some("user-pw")).unwrap();
    assert_eq!(tile(&mut d, 0, 0, 0, 0).pixel(200, 500), [0, 0, 0, 255]);
    // Password is remembered across re-open.
    d.reopen(&[]).unwrap();
    assert_eq!(d.page_count(), 1);
    std::fs::remove_dir_all(&dir).ok();
}
