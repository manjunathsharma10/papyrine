//! Typical and large synthetic documents.

use crate::content::{self, Vocab};
use crate::doc::*;
use crate::pdf::Out;
use crate::rng::Rng;
use std::io::{self, BufWriter, Write};
use std::path::Path;

pub struct Ctx {
    pub jobs: usize,
}

fn create(path: &Path) -> io::Result<Out<BufWriter<std::fs::File>>> {
    Ok(Out::new(BufWriter::with_capacity(
        1 << 20,
        std::fs::File::create(path)?,
    )))
}

fn done(out: Out<BufWriter<std::fs::File>>) -> io::Result<()> {
    out.into_inner().flush()
}

const A4: &str = "0 0 595 842";

fn with_image(
    mut content: Vec<u8>,
    w: usize,
    h: usize,
    draw_w: f64,
    draw_h: f64,
    x: f64,
    y: f64,
) -> Vec<u8> {
    let _ = (w, h);
    content.extend_from_slice(
        format!("q {draw_w:.1} 0 0 {draw_h:.1} {x:.1} {y:.1} cm /Im0 Do Q\n").as_bytes(),
    );
    content
}

pub fn typical_20p(c: &Ctx, path: &Path) -> io::Result<()> {
    let n = 20;
    let vocab = Vocab::new(1, 4000);
    let mut out = create(path)?;
    write_prelude(&mut out, n, A4, "Typical 20 page document")?;
    let produce = |i: usize| {
        let mut r = Rng::new(100 + i as u64);
        let title = format!("Section {} - mixed text and images", i + 1);
        let mut content = content::text_page(&vocab, &mut r, &title, 28, 11, 10.0);
        let (w, h) = (1100, 800);
        let img = content::image_rgb(w, h, 500 + i as u64, 2, 1);
        content = with_image(content, w, h, 480.0, 349.0, 57.0, 60.0);
        PageData {
            content,
            image: Some(Image {
                w,
                h,
                zdata: zimage(&img, 6),
            }),
        }
    };
    par_ordered(n, c.jobs, &produce, |i, p| {
        write_page(&mut out, i, A4, &p, 6)
    })?;
    finish_classic(&mut out, 1)?;
    done(out)
}

pub fn large_2000p_text(c: &Ctx, path: &Path) -> io::Result<()> {
    let n = 2000;
    let vocab = Vocab::new(2, 20000);
    let mut out = create(path)?;
    write_prelude(&mut out, n, A4, "2000 page text document")?;
    let produce = |i: usize| {
        let mut r = Rng::new(10_000 + i as u64);
        let mut content = content::text_page(
            &vocab,
            &mut r,
            &format!("Page {} of 2000", i + 1),
            78,
            16,
            7.0,
        );
        content.extend(content::number_rows(&mut r, 24, 8, 6.0, 54.0, 150.0));
        content.extend(content::vector_drawing(595.0, 580, 40_000 + i as u64));
        PageData {
            content,
            image: None,
        }
    };
    par_ordered(n, c.jobs * 2, &produce, |i, p| {
        write_page(&mut out, i, A4, &p, 6)
    })?;
    finish_classic(&mut out, 2)?;
    done(out)
}

/// 1000 pages, each a full-page 300 dpi RGB raster (2480x3508).
pub fn large_1000p_images(c: &Ctx, path: &Path) -> io::Result<()> {
    let n = 1000;
    let (w, h) = (2480, 3508);
    let vocab = Vocab::new(3, 500);
    let mut out = create(path)?;
    write_prelude(&mut out, n, A4, "1000 page scanned-style document")?;
    let produce = |i: usize| {
        let mut r = Rng::new(20_000 + i as u64);
        let img = content::image_rgb(w, h, 30_000 + i as u64, 0, 64);
        let mut content = with_image(Vec::new(), w, h, 595.0, 842.0, 0.0, 0.0);
        content.extend(content::text_page(
            &vocab,
            &mut r,
            &format!("Scan {}", i + 1),
            0,
            0,
            8.0,
        ));
        PageData {
            content,
            image: Some(Image {
                w,
                h,
                zdata: zimage(&img, 1),
            }),
        }
    };
    par_ordered(n, c.jobs, &produce, |i, p| {
        write_page(&mut out, i, A4, &p, 6)
    })?;
    finish_classic(&mut out, 3)?;
    done(out)
}

/// One 5 m x 5 m page (14173 pt) with a dense vector drawing.
pub fn large_single_page(_c: &Ctx, path: &Path) -> io::Result<()> {
    let side = 14173.0;
    let mut out = create(path)?;
    let mb = format!("0 0 {side} {side}");
    write_prelude(&mut out, 1, &mb, "5 m x 5 m single page")?;
    let content = content::vector_drawing(side, 400_000, 7);
    write_page(
        &mut out,
        0,
        &mb,
        &PageData {
            content,
            image: None,
        },
        6,
    )?;
    finish_classic(&mut out, 4)?;
    done(out)
}

const ITEMS: u32 = 1_600_000;
const FIRST_ITEM: u32 = 100;

fn item_body(i: u32) -> Vec<u8> {
    match i % 4 {
        0 => format!(
            "<< /Type /Item /Index {i} /Name (item-{i}) /Weight {}.5 >>",
            i % 977
        ),
        1 => format!("{}", i.wrapping_mul(2654435761) % 1_000_003),
        2 => format!("(payload {i:08x})"),
        _ => format!("[{i} {} /Tag{}]", i % 7, i % 13),
    }
    .into_bytes()
}

/// >= 1.5 million indirect objects reachable from the catalog through a two-level array tree.
pub fn large_objects(_c: &Ctx, path: &Path, packed: bool) -> io::Result<()> {
    let mut out = create(path)?;
    out.header(if packed { "1.7" } else { "1.5" })?;
    let groups = ITEMS.div_ceil(1000);
    let group_base = FIRST_ITEM + ITEMS;
    let root_arr = group_base + groups;
    let put = |out: &mut Out<BufWriter<std::fs::File>>, n: u32, body: Vec<u8>| -> io::Result<()> {
        if packed {
            out.packed(n, body)
        } else {
            out.obj(n, &body)
        }
    };
    if packed {
        out.set_object_stream_numbers(root_arr + 1, 200);
    }
    out.obj(
        1,
        format!("<< /Type /Catalog /Pages 2 0 R /PapyrineItems {root_arr} 0 R >>").as_bytes(),
    )?;
    out.obj(2, b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>")?;
    out.obj(3, b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")?;
    out.obj(4, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Contents 5 0 R /Resources << /Font << /F1 3 0 R >> >> >>")?;
    out.stream(
        5,
        "",
        b"BT /F1 24 Tf 72 700 Td (1,600,000 indirect objects) Tj ET\n",
    )?;
    out.obj(
        6,
        format!("<< /Producer (Papyrine gen-corpus) /CreationDate ({INFO_DATE}) >>").as_bytes(),
    )?;
    for g in 0..groups {
        let lo = g * 1000;
        let hi = ((g + 1) * 1000).min(ITEMS);
        let refs: Vec<String> = (lo..hi)
            .map(|i| format!("{} 0 R", FIRST_ITEM + i))
            .collect();
        put(
            &mut out,
            group_base + g,
            format!("[{}]", refs.join(" ")).into_bytes(),
        )?;
        for i in lo..hi {
            put(&mut out, FIRST_ITEM + i, item_body(i))?;
        }
    }
    let refs: Vec<String> = (0..groups)
        .map(|g| format!("{} 0 R", group_base + g))
        .collect();
    put(
        &mut out,
        root_arr,
        format!("[{}]", refs.join(" ")).into_bytes(),
    )?;
    let extra = format!("/Info 6 0 R {}", fixed_id(5));
    if packed {
        let xnum = root_arr + 1 + ITEMS.div_ceil(200) + 20;
        out.finish_xref_stream(xnum, 1, &extra)?;
    } else {
        out.finish_classic(1, &extra)?;
    }
    done(out)
}
