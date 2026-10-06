//! "Print as image": render every page of the print PDF with PDFium at the printer's DPI and
//! wrap the bitmaps in a new PDF (one image per page, Flate-compressed as they are produced so
//! a long job never holds raw bitmaps). For files Quartz or CUPS mishandle (ADR-025).

use crate::{Error, Result, Stage};
use papyrine_core::CancelToken;
use papyrine_cos::{Document, EncryptionMode, ObjectStreams, StreamMode, WriteOptions};
use papyrine_render::{Library, bytes_from_vec};

/// Longest bitmap edge we are willing to allocate (pixels).
const MAX_EDGE_PX: u32 = 16_384;

fn pdf_err(e: papyrine_cos::Error) -> Error {
    Error::Pdf(e.to_string())
}

fn render_err(e: papyrine_render::Error) -> Error {
    match e {
        papyrine_render::Error::Cancelled => Error::Cancelled,
        other => Error::Render(other.to_string()),
    }
}

/// Render `pdf` at `dpi` and return a raster-only PDF with the same page sizes.
pub fn rasterize(
    pdf: &[u8],
    dpi: u32,
    cancel: &CancelToken,
    progress: &dyn Fn(Stage, u64, u64),
) -> Result<Vec<u8>> {
    let dpi = dpi.clamp(72, 1200);
    let lib = Library::global().map_err(render_err)?;
    let mut src = papyrine_render::Document::open(&lib, bytes_from_vec(pdf.to_vec()), &[], None)
        .map_err(render_err)?;
    let n = src.page_count();
    let out = Document::new_empty().map_err(pdf_err)?;
    let flate = out.new_name("FlateDecode").map_err(pdf_err)?;
    for i in 0..n {
        cancel.check().map_err(|_| Error::Cancelled)?;
        progress(Stage::Rendering, i as u64, n as u64);
        let (wp, hp) = src.page_size(i).map_err(render_err)?;
        let edge = ((wp.max(hp) as f64 / 72.0) * dpi as f64).round() as u32;
        let tile = src
            .render_preview(i, edge.clamp(1, MAX_EDGE_PX), &mut || cancel.is_cancelled())
            .map_err(render_err)?;
        let mut rgb = Vec::with_capacity(tile.width as usize * tile.height as usize * 3);
        let (quads, _) = tile.rgba.as_chunks::<4>();
        for px in quads {
            rgb.extend_from_slice(&px[..3]);
        }
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&rgb, 6);
        drop(rgb);

        let img = out.new_stream(b"").map_err(pdf_err)?;
        img.stream_replace(&packed, Some(&flate), None)
            .map_err(pdf_err)?;
        let d = img.stream_dict().map_err(pdf_err)?;
        let name = |s: &str| out.new_name(s).map_err(pdf_err);
        d.dict_set("Type", &name("XObject")?).map_err(pdf_err)?;
        d.dict_set("Subtype", &name("Image")?).map_err(pdf_err)?;
        d.dict_set("Width", &out.new_int(tile.width as i64))
            .map_err(pdf_err)?;
        d.dict_set("Height", &out.new_int(tile.height as i64))
            .map_err(pdf_err)?;
        d.dict_set("ColorSpace", &name("DeviceRGB")?)
            .map_err(pdf_err)?;
        d.dict_set("BitsPerComponent", &out.new_int(8))
            .map_err(pdf_err)?;
        let img = out.make_indirect(&img).map_err(pdf_err)?;

        let content = format!("q\n{wp} 0 0 {hp} 0 0 cm\n/Im0 Do\nQ\n");
        let contents = out.new_stream(content.as_bytes()).map_err(pdf_err)?;
        let contents = out.make_indirect(&contents).map_err(pdf_err)?;
        let page = out
            .parse_object(format!(
                "<< /Type /Page /MediaBox [0 0 {wp} {hp}] /Resources << /XObject << >> >> >>"
            ))
            .map_err(pdf_err)?;
        page.dict_get("Resources")
            .and_then(|r| r.dict_get("XObject"))
            .and_then(|x| x.dict_set("Im0", &img))
            .map_err(pdf_err)?;
        page.dict_set("Contents", &contents).map_err(pdf_err)?;
        let page = out.make_indirect(&page).map_err(pdf_err)?;
        out.add_page(&page, false).map_err(pdf_err)?;
    }
    progress(Stage::Rendering, n as u64, n as u64);
    let written = out
        .write(&WriteOptions {
            object_streams: ObjectStreams::Disable,
            stream_data: StreamMode::Preserve,
            encryption: EncryptionMode::Decrypt,
            ..WriteOptions::default()
        })
        .map_err(pdf_err)?;
    Ok(written.into_vec())
}
