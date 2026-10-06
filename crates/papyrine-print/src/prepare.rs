//! The shared print-PDF pipeline (ARCHITECTURE section 11.2): the page subset, annotations
//! filtered by the `/Print` flag and the "print comments" option, form appearances generated,
//! all through qpdf so damaged files print the way they display.
//!
//! [`build_print_pdf`] runs where the document is parsed (the engine); the result is a plain,
//! unencrypted, fully normalized PDF the back ends only ever hand to the OS.
//! [`impose`] then places every page on a known sheet with the layout math, for back ends
//! (Linux/CUPS) whose OS API does not scale for us.

use crate::layout::{Layout, Sheet, place_page};
use crate::{Error, PrintOptions, Result, Stage};
use papyrine_core::{CancelToken, Rect};
use papyrine_cos::{
    Document, FlattenOptions, Object, ObjectKind, OpenOptions, WriteOptions, annotation_flags,
};

/// The normalized print PDF plus what it contains.
#[derive(Debug, Clone)]
pub struct PrintPdf {
    pub bytes: Vec<u8>,
    pub page_count: usize,
    /// Zero-based indices of the source pages, in print order.
    pub source_pages: Vec<usize>,
    /// Pages that had at least one annotation dropped by the print filter.
    pub pages_with_dropped_annotations: usize,
}

fn cos(e: papyrine_cos::Error) -> Error {
    match e {
        papyrine_cos::Error::InvalidPassword => Error::PasswordRequired,
        other => Error::Pdf(other.to_string()),
    }
}

/// qpdf's writer copies the trailer's keys and only refreshes `/Size`, so a trailer that lacks
/// one (files whose xref was rebuilt, or hand-written ones) stays without. Give it a
/// placeholder; the writer overwrites it with the real value.
fn ensure_trailer_size(doc: &Document) -> Result<()> {
    let trailer = doc.trailer().map_err(cos)?;
    if !trailer.dict_has("Size").map_err(cos)? {
        let n = doc.object_count().map_err(cos)? as i64 + 1;
        trailer.dict_set("Size", &doc.new_int(n)).map_err(cos)?;
    }
    Ok(())
}

fn int_of(o: &Object) -> Option<i64> {
    match o.kind().ok()? {
        ObjectKind::Integer => o.as_int().ok(),
        ObjectKind::Real => o.as_f64().ok().map(|v| v as i64),
        _ => None,
    }
}

fn name_of(o: &Object) -> Option<String> {
    (o.kind().ok()? == ObjectKind::Name)
        .then(|| {
            o.name()
                .ok()
                .map(|n| String::from_utf8_lossy(&n).into_owned())
        })
        .flatten()
}

/// Should this annotation appear on paper?
fn prints(annot: &Object, print_comments: bool) -> bool {
    if annot.kind().ok() != Some(ObjectKind::Dictionary) {
        return false;
    }
    let flags = annot
        .dict_get("F")
        .ok()
        .as_ref()
        .and_then(int_of)
        .unwrap_or(0) as u32;
    if flags & annotation_flags::PRINT == 0 || flags & (annotation_flags::HIDDEN) != 0 {
        return false;
    }
    let subtype = annot
        .dict_get("Subtype")
        .ok()
        .as_ref()
        .and_then(name_of)
        .unwrap_or_default();
    match subtype.as_str() {
        // Popups are UI for their parent; links draw nothing worth printing.
        "Popup" | "Link" => false,
        // Form values always print (the "form values included" option of the basic scope).
        "Widget" => true,
        _ => print_comments,
    }
}

/// Rewrite one page's `/Annots` to the printable subset. Returns how many were dropped.
fn filter_annotations(page: &Object, print_comments: bool) -> Result<usize> {
    let annots = page.dict_get("Annots").map_err(cos)?;
    if annots.kind().map_err(cos)? != ObjectKind::Array {
        return Ok(0);
    }
    let mut dropped = 0;
    // Back to front so removals do not shift pending indices.
    for i in (0..annots.array_len().map_err(cos)?).rev() {
        let a = annots.array_get(i).map_err(cos)?;
        if !prints(&a, print_comments) {
            annots.array_remove(i).map_err(cos)?;
            dropped += 1;
        }
    }
    Ok(dropped)
}

/// Build the print PDF for `doc_bytes` (any PDF qpdf can open, repaired when damaged).
pub fn build_print_pdf(
    doc_bytes: &[u8],
    password: Option<&str>,
    opts: &PrintOptions,
    cancel: &CancelToken,
    progress: &dyn Fn(Stage, u64, u64),
) -> Result<PrintPdf> {
    let open = match password {
        Some(p) => OpenOptions::with_password(p),
        None => OpenOptions::default(),
    };
    let doc = Document::open_bytes(Vec::from(doc_bytes), &open).map_err(cos)?;
    if let Some(enc) = doc.encryption().map_err(cos)?
        && !enc.owner_password_matched
        && !enc.allow_print_low_res
        && !enc.allow_print_high_res
    {
        return Err(Error::PrintNotPermitted);
    }
    let selected = opts.pages.resolve(doc.page_count().map_err(cos)?)?;
    let total = selected.len() as u64 + 2;
    progress(Stage::Preparing, 0, total);
    cancel.check().map_err(|_| Error::Cancelled)?;

    // Keep only the selected pages (descendants of one Pages tree after the push-down).
    doc.push_inherited_page_attributes().map_err(cos)?;
    let pages = doc.pages().map_err(cos)?;
    let keep: std::collections::HashSet<usize> = selected.iter().copied().collect();
    let kept: Vec<&Object> = selected.iter().map(|&i| &pages[i]).collect();
    for (i, p) in pages.iter().enumerate().rev() {
        if !keep.contains(&i) {
            doc.remove_page(p).map_err(cos)?;
        }
    }

    // Form appearances first, so flattening bakes current values.
    if doc.has_acroform().map_err(cos)? && doc.need_appearances().map_err(cos)? {
        doc.generate_form_appearances().map_err(cos)?;
    }
    let mut touched = 0;
    for (n, p) in kept.iter().enumerate() {
        cancel.check().map_err(|_| Error::Cancelled)?;
        if filter_annotations(p, opts.print_comments)? > 0 {
            touched += 1;
        }
        progress(Stage::Preparing, n as u64 + 1, total);
    }
    // Everything left prints: bake it into the content, then drop what flattening left behind
    // (links without appearances, widget remnants) and every interactive entry point.
    doc.flatten_annotations(FlattenOptions::printable())
        .map_err(cos)?;
    for p in &kept {
        p.dict_remove("Annots").map_err(cos)?;
        p.dict_remove("AA").map_err(cos)?;
    }
    let root = doc.root().map_err(cos)?;
    for key in [
        "AcroForm",
        "OpenAction",
        "AA",
        "Names",
        "Dests",
        "Outlines",
        "StructTreeRoot",
        "PageLabels",
        "Threads",
    ] {
        root.dict_remove(key).map_err(cos)?;
    }
    cancel.check().map_err(|_| Error::Cancelled)?;

    ensure_trailer_size(&doc)?;
    let out = doc
        .write(&WriteOptions {
            encryption: papyrine_cos::EncryptionMode::Decrypt,
            ..WriteOptions::default()
        })
        .map_err(cos)?;
    let bytes = out.into_vec();
    progress(Stage::Preparing, total, total);
    let page_count = selected.len();
    let bytes = if opts.as_image {
        crate::raster::rasterize(&bytes, opts.image_dpi, cancel, progress)?
    } else {
        bytes
    };
    Ok(PrintPdf {
        bytes,
        page_count,
        source_pages: selected,
        pages_with_dropped_annotations: touched,
    })
}

fn num(v: f64) -> String {
    let s = format!("{v:.5}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

fn page_box(page: &Object) -> Option<Rect> {
    let read = |key: &str| -> Option<Rect> {
        let a = page.dict_get(key).ok()?;
        if a.kind().ok()? != ObjectKind::Array || a.array_len().ok()? != 4 {
            return None;
        }
        let mut v = [0.0; 4];
        for (i, slot) in v.iter_mut().enumerate() {
            *slot = a.array_get(i).ok()?.as_f64().ok()?;
        }
        Some(Rect::from_pdf_array(v).normalized())
    };
    let media = read("MediaBox").unwrap_or(Rect::new(0.0, 0.0, 612.0, 792.0));
    match read("CropBox") {
        Some(c) => c.intersect(&media).or(Some(media)),
        None => Some(media),
    }
}

/// Place every page of `print_pdf` on `sheet` (paper size and margins in points) with the
/// layout math. Each page becomes a sheet-sized page whose content is the original content
/// wrapped in a clip and a `cm`; `/Rotate` is folded into the matrix.
pub fn impose(
    print_pdf: &[u8],
    sheet: &Sheet,
    layout: &Layout,
    cancel: &CancelToken,
) -> Result<Vec<u8>> {
    let doc = Document::open_bytes(Vec::from(print_pdf), &OpenOptions::default()).map_err(cos)?;
    doc.push_inherited_page_attributes().map_err(cos)?;
    let area = sheet.printable_area();
    for page in doc.pages().map_err(cos)? {
        cancel.check().map_err(|_| Error::Cancelled)?;
        let bx = page_box(&page).unwrap_or(Rect::new(0.0, 0.0, 612.0, 792.0));
        let rotate = page
            .dict_get("Rotate")
            .ok()
            .as_ref()
            .and_then(int_of)
            .unwrap_or(0) as i32;
        let pl = place_page(bx, rotate, area, layout);
        let m = pl.matrix;
        let prefix = format!(
            "q\n{} {} {} {} {} {} cm\n{} {} {} {} re W n\n",
            num(m.a),
            num(m.b),
            num(m.c),
            num(m.d),
            num(m.e),
            num(m.f),
            num(bx.x0),
            num(bx.y0),
            num(bx.width()),
            num(bx.height()),
        );
        let wrap = |data: &[u8]| -> Result<Object> {
            let s = doc.new_stream(data).map_err(cos)?;
            doc.make_indirect(&s).map_err(cos)
        };
        let contents = doc.new_array();
        contents
            .array_push(&wrap(prefix.as_bytes())?)
            .map_err(cos)?;
        let old = page.dict_get("Contents").map_err(cos)?;
        match old.kind().map_err(cos)? {
            ObjectKind::Stream => contents.array_push(&old).map_err(cos)?,
            ObjectKind::Array => {
                for o in old.array_items().map_err(cos)? {
                    contents.array_push(&o).map_err(cos)?;
                }
            }
            _ => {}
        }
        contents.array_push(&wrap(b"\nQ\n")?).map_err(cos)?;
        page.dict_set("Contents", &contents).map_err(cos)?;
        let media = doc
            .parse_object(format!("[0 0 {} {}]", num(sheet.width), num(sheet.height)))
            .map_err(cos)?;
        page.dict_set("MediaBox", &media).map_err(cos)?;
        for key in ["CropBox", "TrimBox", "BleedBox", "ArtBox", "Rotate"] {
            page.dict_remove(key).map_err(cos)?;
        }
    }
    ensure_trailer_size(&doc)?;
    let out = doc.write(&WriteOptions::default()).map_err(cos)?;
    Ok(out.into_vec())
}
