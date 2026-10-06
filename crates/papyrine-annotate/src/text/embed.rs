//! Font subsetting and the PDF font objects: Type0 / CIDFontType2 (or CIDFontType0 for CFF)
//! with Identity-H and a ToUnicode CMap, so the text stays extractable.

use std::collections::BTreeMap;
use std::io::Write;

use papyrine_cos::{Document, Object};
use papyrine_ops::{Error, Result};

use super::layout::Layout;

/// A subset of one face ready to become PDF objects.
pub struct PreparedFont {
    /// Resource name in the appearance stream (`F0`, `F1`...).
    pub resource: String,
    /// Index into [`Layout::faces`].
    pub face: usize,
    pub base_font: String,
    pub subset: Vec<u8>,
    /// OpenType with CFF outlines (FontFile3) rather than TrueType (FontFile2).
    pub cff: bool,
    /// Original glyph id -> glyph id in the subset (which is also the CID).
    pub remap: BTreeMap<u16, u16>,
    /// CID -> advance in 1/1000 em.
    pub widths: BTreeMap<u16, i64>,
    pub to_unicode: BTreeMap<u16, String>,
    pub bbox: [i64; 4],
    pub ascent: i64,
    pub descent: i64,
    pub cap_height: i64,
    pub italic_angle: f64,
    pub flags: i64,
    pub stem_v: i64,
}

fn fnv(data: impl IntoIterator<Item = u8>) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn subset_tag(key: &str, gids: impl Iterator<Item = u16>) -> String {
    let mut h = fnv(key.bytes().chain(gids.flat_map(u16::to_le_bytes)));
    (0..6)
        .map(|_| {
            let c = (b'A' + (h % 26) as u8) as char;
            h /= 26;
            c
        })
        .collect()
}

/// Subset every face the layout used.
pub fn prepare(layout: &Layout) -> Result<Vec<PreparedFont>> {
    let used = layout.used_glyphs();
    let mut out = Vec::new();
    for (fi, glyphs) in used.iter().enumerate() {
        if glyphs.is_empty() {
            continue;
        }
        let face = &layout.faces[fi];
        let ttf = face
            .ttf()
            .ok_or_else(|| Error::invalid(format!("font {} cannot be parsed", face.name)))?;
        let mut remapper = subsetter::GlyphRemapper::new();
        remapper.remap(0);
        for &g in glyphs.keys() {
            remapper.remap(g);
        }
        let subset = subsetter::subset(face.bytes(), face.index, &remapper)
            .map_err(|e| Error::invalid(format!("cannot subset font {}: {e}", face.name)))?;
        let cff = subset.starts_with(b"OTTO");
        let upem = f64::from(ttf.units_per_em().max(1));
        let k = 1000.0 / upem;
        let mut remap = BTreeMap::new();
        let mut widths = BTreeMap::new();
        let mut to_unicode = BTreeMap::new();
        for (&g, text) in glyphs {
            let cid = remapper.get(g).unwrap_or(0);
            remap.insert(g, cid);
            let adv = ttf
                .glyph_hor_advance(rustybuzz::ttf_parser::GlyphId(g))
                .unwrap_or(0);
            widths.insert(cid, (f64::from(adv) * k).round() as i64);
            if !text.is_empty() && g != 0 {
                to_unicode.insert(cid, text.clone());
            }
        }
        let bb = ttf.global_bounding_box();
        let tag = subset_tag(&face.key, glyphs.keys().copied());
        let ps = face.name.replace(' ', "");
        let mut flags = 4; // Symbolic: the encoding is Identity, not a standard Latin one
        if ttf.is_monospaced() {
            flags |= 1;
        }
        out.push(PreparedFont {
            resource: format!("F{}", out.len()),
            face: fi,
            base_font: format!("{tag}+{ps}"),
            subset,
            cff,
            remap,
            widths,
            to_unicode,
            bbox: [
                (f64::from(bb.x_min) * k).round() as i64,
                (f64::from(bb.y_min) * k).round() as i64,
                (f64::from(bb.x_max) * k).round() as i64,
                (f64::from(bb.y_max) * k).round() as i64,
            ],
            ascent: (f64::from(ttf.ascender()) * k).round() as i64,
            descent: (f64::from(ttf.descender()) * k).round() as i64,
            cap_height: ttf
                .capital_height()
                .map_or(700, |c| (f64::from(c) * k).round() as i64),
            italic_angle: f64::from(ttf.italic_angle()),
            flags,
            stem_v: if ttf.is_bold() { 140 } else { 80 },
        });
    }
    Ok(out)
}

fn utf16_hex(s: &str) -> String {
    s.encode_utf16().map(|u| format!("{u:04X}")).collect()
}

/// A ToUnicode CMap mapping 2-byte CIDs to text.
pub fn to_unicode_cmap(map: &BTreeMap<u16, String>) -> String {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
         /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
         1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries: Vec<_> = map.iter().collect();
    for chunk in entries.chunks(100) {
        s.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (cid, text) in chunk {
            s.push_str(&format!("<{cid:04X}> <{}>\n", utf16_hex(text)));
        }
        s.push_str("endbfchar\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    s
}

/// Zlib-compress `data`.
pub fn deflate(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    // Writing to a Vec cannot fail.
    let _ = e.write_all(data);
    e.finish().unwrap_or_default()
}

/// A new indirect stream holding `data`, Flate-compressed (with `extra` dictionary entries).
pub fn new_flate_stream(doc: &Document, data: &[u8], extra: &[(&str, Object)]) -> Result<Object> {
    let s = doc.new_stream(b"")?;
    let z = deflate(data);
    s.stream_replace(&z, Some(&doc.new_name("FlateDecode")?), None)?;
    for (k, v) in extra {
        s.dict_set(k, v)?;
    }
    Ok(s)
}

fn num_array(doc: &Document, v: &[i64]) -> Result<Object> {
    let a = doc.new_array();
    for n in v {
        a.array_push(&doc.new_int(*n))?;
    }
    Ok(a)
}

/// Create the font objects for `f`; returns the indirect Type0 font.
pub fn materialize(doc: &Document, f: &PreparedFont) -> Result<Object> {
    let file = if f.cff {
        new_flate_stream(doc, &f.subset, &[("Subtype", doc.new_name("OpenType")?)])?
    } else {
        new_flate_stream(
            doc,
            &f.subset,
            &[("Length1", doc.new_int(f.subset.len() as i64))],
        )?
    };
    let fd = doc.new_dict();
    fd.dict_set("Type", &doc.new_name("FontDescriptor")?)?;
    fd.dict_set("FontName", &doc.new_name(&f.base_font)?)?;
    fd.dict_set("Flags", &doc.new_int(f.flags))?;
    fd.dict_set("FontBBox", &num_array(doc, &f.bbox)?)?;
    fd.dict_set("ItalicAngle", &doc.new_real(f.italic_angle)?)?;
    fd.dict_set("Ascent", &doc.new_int(f.ascent))?;
    fd.dict_set("Descent", &doc.new_int(f.descent))?;
    fd.dict_set("CapHeight", &doc.new_int(f.cap_height))?;
    fd.dict_set("StemV", &doc.new_int(f.stem_v))?;
    fd.dict_set(if f.cff { "FontFile3" } else { "FontFile2" }, &file)?;
    let fd = doc.make_indirect(&fd)?;

    // /W as individual entries: cid [w]. Compact enough for the few glyphs of one text box.
    let w = doc.new_array();
    for (cid, width) in &f.widths {
        w.array_push(&doc.new_int(i64::from(*cid)))?;
        let inner = doc.new_array();
        inner.array_push(&doc.new_int(*width))?;
        w.array_push(&inner)?;
    }
    let cid_font = doc.new_dict();
    cid_font.dict_set("Type", &doc.new_name("Font")?)?;
    cid_font.dict_set(
        "Subtype",
        &doc.new_name(if f.cff {
            "CIDFontType0"
        } else {
            "CIDFontType2"
        })?,
    )?;
    cid_font.dict_set("BaseFont", &doc.new_name(&f.base_font)?)?;
    cid_font.dict_set(
        "CIDSystemInfo",
        &doc.parse_object("<< /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>")?,
    )?;
    cid_font.dict_set("FontDescriptor", &fd)?;
    cid_font.dict_set("DW", &doc.new_int(1000))?;
    cid_font.dict_set("W", &w)?;
    if !f.cff {
        cid_font.dict_set("CIDToGIDMap", &doc.new_name("Identity")?)?;
    }
    let cid_font = doc.make_indirect(&cid_font)?;

    let tu = new_flate_stream(doc, to_unicode_cmap(&f.to_unicode).as_bytes(), &[])?;
    let desc = doc.new_array();
    desc.array_push(&cid_font)?;
    let font = doc.new_dict();
    font.dict_set("Type", &doc.new_name("Font")?)?;
    font.dict_set("Subtype", &doc.new_name("Type0")?)?;
    font.dict_set("BaseFont", &doc.new_name(&f.base_font)?)?;
    font.dict_set("Encoding", &doc.new_name("Identity-H")?)?;
    font.dict_set("DescendantFonts", &desc)?;
    font.dict_set("ToUnicode", &tu)?;
    Ok(doc.make_indirect(&font)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::TextStyle;
    use crate::text::layout::layout;

    #[test]
    fn subset_is_small_and_complete() {
        let l = layout("Hello, Привет, Γειά!", &TextStyle::default(), 500.0);
        let fonts = prepare(&l).unwrap();
        assert_eq!(fonts.len(), 1);
        let f = &fonts[0];
        assert!(!f.cff);
        assert!(f.subset.len() < 30_000, "{}", f.subset.len());
        // Every used glyph is mapped, has a width and (except notdef) a Unicode value.
        assert!(f.to_unicode.values().any(|t| t == "П"));
        assert!(f.widths.len() >= f.to_unicode.len());
        let sub = rustybuzz::ttf_parser::Face::parse(&f.subset, 0).unwrap();
        assert!(usize::from(sub.number_of_glyphs()) >= f.remap.len());
    }

    #[test]
    fn cmap_formats_surrogates() {
        let mut m = BTreeMap::new();
        m.insert(3, "\u{1F600}".to_string());
        m.insert(4, "ffi".to_string());
        let s = to_unicode_cmap(&m);
        assert!(s.contains("<0003> <D83DDE00>"), "{s}");
        assert!(s.contains("<0004> <006600660069>"));
    }
}
