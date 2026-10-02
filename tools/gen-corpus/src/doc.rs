//! Standard multi-page document layout used by the typical and large generators.

use crate::pdf::{Out, flate};
use std::io::{self, Write};

pub struct Image {
    pub w: usize,
    pub h: usize,
    pub zdata: Vec<u8>,
}

pub struct PageData {
    pub content: Vec<u8>,
    pub image: Option<Image>,
}

pub const INFO_DATE: &str = "D:20260101000000Z";
pub const FONTS: &str = "/F1 3 0 R /F2 4 0 R /F3 5 0 R";

pub fn fixed_id(tag: u8) -> String {
    let h: String = (0..16)
        .map(|i| format!("{:02x}", (i as u8).wrapping_mul(17).wrapping_add(tag)))
        .collect();
    format!("/ID [<{h}> <{h}>]")
}

pub fn write_prelude<W: Write>(
    out: &mut Out<W>,
    n_pages: usize,
    mediabox: &str,
    title: &str,
) -> io::Result<()> {
    out.header("1.7")?;
    out.obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")?;
    let kids: String = (0..n_pages)
        .map(|i| format!("{} 0 R", 10 + i * 3))
        .collect::<Vec<_>>()
        .join(" ");
    let _ = mediabox;
    out.obj(
        2,
        format!("<< /Type /Pages /Kids [{kids}] /Count {n_pages} >>").as_bytes(),
    )?;
    out.obj(
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    )?;
    out.obj(
        4,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>",
    )?;
    out.obj(
        5,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>",
    )?;
    out.obj(
        6,
        format!("<< /Title ({title}) /Producer (Papyrine gen-corpus) /CreationDate ({INFO_DATE}) /ModDate ({INFO_DATE}) >>").as_bytes(),
    )
}

pub fn write_page<W: Write>(
    out: &mut Out<W>,
    i: usize,
    mediabox: &str,
    p: &PageData,
    level: u8,
) -> io::Result<()> {
    let base = (10 + i * 3) as u32;
    let mut xobj = String::new();
    if let Some(im) = &p.image {
        let dict = format!(
            "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode",
            im.w, im.h
        );
        out.stream(base + 2, &dict, &im.zdata)?;
        xobj = format!("/XObject << /Im0 {} 0 R >>", base + 2);
    }
    out.obj(
        base,
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [{mediabox}] /Contents {} 0 R /Resources << /Font << {FONTS} >> {xobj} >> >>",
            base + 1
        )
        .as_bytes(),
    )?;
    out.flate_stream(base + 1, "", &p.content, level)
}

/// Run `produce(i)` on up to `jobs` threads, delivering results to `consume` strictly in order.
pub fn par_ordered<T: Send>(
    n: usize,
    jobs: usize,
    produce: &(dyn Fn(usize) -> T + Sync),
    mut consume: impl FnMut(usize, T) -> io::Result<()>,
) -> io::Result<()> {
    let jobs = jobs.max(1);
    let mut start = 0;
    while start < n {
        let end = (start + jobs).min(n);
        let results: Vec<T> = std::thread::scope(|s| {
            let hs: Vec<_> = (start..end).map(|i| s.spawn(move || produce(i))).collect();
            hs.into_iter()
                .map(|h| h.join().expect("worker panicked"))
                .collect()
        });
        for (k, r) in results.into_iter().enumerate() {
            consume(start + k, r)?;
        }
        start = end;
    }
    Ok(())
}

pub fn finish_classic<W: Write>(out: &mut Out<W>, tag: u8) -> io::Result<()> {
    out.finish_classic(1, &format!("/Info 6 0 R {}", fixed_id(tag)))
        .map(|_| ())
}

pub fn zimage(raw: &[u8], level: u8) -> Vec<u8> {
    flate(raw, level)
}
