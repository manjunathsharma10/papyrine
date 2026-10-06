//! Fonts for appearance generation: resolve the font a field's `/DA` names, measure text with it
//! and encode text into the bytes its content stream must show.
//!
//! Two families are supported:
//!
//! * simple fonts (Type1, TrueType): `/Encoding` (base encoding plus `/Differences`),
//!   `/Widths` or the Core 14 metrics, no font program needed;
//! * Type0 fonts with an `Identity-H` encoding and an embedded TrueType/OpenType program.
//!   Text is shaped with rustybuzz (ligatures, marks, right-to-left), glyph ids become the
//!   codes, and the difference between the shaper's advance and the font's `/W` entry goes into
//!   `TJ` adjustments so the glyphs land where the shaper put them.
//!
//! Anything else (CFF-only CID fonts, predefined CMaps, text the font has no glyph for) is
//! reported as unencodable so that the caller keeps `/NeedAppearances` set.

mod base14_data;

use std::collections::HashMap;

use papyrine_content::TextItem;
use papyrine_cos::{DecodeLevel, Document, Object, ObjectKind};

use crate::cosx;
use crate::error::{FillError, Result};

pub use base14_data::{Base14, GLYPH_NAMES, ZAPF_BBOX};

/// Index into [`base14_data::BASE14`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base14Id(pub usize);

pub const HELVETICA: Base14Id = Base14Id(0);
pub const ZAPF_DINGBATS: Base14Id = Base14Id(13);

pub fn base14(id: Base14Id) -> &'static Base14 {
    &base14_data::BASE14[id.0]
}

/// Map a `/BaseFont` name (subset prefix, Arial, TimesNewRoman, ... aliases included) to a Core
/// 14 font, the way viewers substitute non-embedded fonts.
pub fn base14_for_name(base_font: &str) -> Base14Id {
    let n = base_font
        .split_once('+')
        .filter(|(p, _)| p.len() == 6 && p.bytes().all(|b| b.is_ascii_uppercase()))
        .map_or(base_font, |(_, r)| r);
    let l = n.to_ascii_lowercase();
    if l.contains("zapf") || l == "zadb" {
        return ZAPF_DINGBATS;
    }
    if l.contains("symbol") {
        return Base14Id(12);
    }
    let bold =
        l.contains("bold") || l.contains("black") || l.contains("heavy") || l.ends_with("bd");
    let italic = l.contains("italic") || l.contains("oblique") || l.contains("slanted");
    let serif = [
        "times", "georgia", "garamond", "palatino", "cambria", "minion",
    ]
    .iter()
    .any(|n| l.contains(n))
        || l == "tiro"
        || (l.contains("serif") && !l.contains("sans"));
    let family = if l.contains("cour") || l.contains("mono") || l.contains("consolas") {
        2
    } else if serif {
        1
    } else {
        0
    };
    let style = match (bold, italic) {
        (false, false) => 0,
        (true, false) => 1,
        (false, true) => 2,
        (true, true) => 3,
    };
    // Helvetica: R, B, O, BO. Times: R, B, I, BI. Courier: R, B, O, BO.
    Base14Id(family * 4 + style)
}

/// WinAnsi byte for a Unicode scalar (used to look up Core 14 widths by character).
fn winansi_byte(cp: u32) -> Option<u8> {
    base14_data::WIN_ANSI
        .iter()
        .position(|&u| u32::from(u) == cp && cp >= 0x20)
        .map(|b| b as u8)
}

#[derive(Clone)]
pub struct SimpleFont {
    to_unicode: [u32; 256],
    from_unicode: HashMap<u32, u8>,
    widths: [f64; 256],
}

#[derive(Clone)]
pub struct CidFont {
    program: Vec<u8>,
    dw: f64,
    w: HashMap<u16, f64>,
    /// gid -> cid when `/CIDToGIDMap` is a stream; `None` means identity.
    gid_to_cid: Option<HashMap<u16, u16>>,
}

#[derive(Clone)]
pub enum FontImpl {
    Simple(Box<SimpleFont>),
    Cid(Box<CidFont>),
}

#[derive(Clone)]
pub struct FieldFont {
    /// Resource name used by the `/DA` string (no slash).
    pub res_name: String,
    /// The font object in the document; `None` when it has to be synthesised.
    pub obj: Option<Object>,
    pub ascent: f64,
    pub descent: f64,
    pub cap_height: f64,
    pub imp: FontImpl,
    /// Core 14 identity when the font is (or is substituted by) one.
    pub core14: Option<Base14Id>,
}

/// Bytes to show plus the advance they produce, in 1/1000 em.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub items: Vec<TextItem>,
    pub width: f64,
}

impl FieldFont {
    /// Helvetica with WinAnsi encoding, referenced under `res_name`.
    pub fn synthetic_helvetica(res_name: &str) -> FieldFont {
        let b = base14(HELVETICA);
        FieldFont {
            res_name: res_name.to_string(),
            obj: None,
            ascent: f64::from(b.ascent),
            descent: f64::from(b.descent),
            cap_height: f64::from(b.cap_height),
            imp: FontImpl::Simple(Box::new(simple_from_base14(HELVETICA))),
            core14: Some(HELVETICA),
        }
    }

    /// The ZapfDingbats font used for check marks, referenced under `res_name`.
    pub fn zapf(res_name: &str) -> FieldFont {
        let b = base14(ZAPF_DINGBATS);
        let mut widths = [0.0; 256];
        let mut to_unicode = [0u32; 256];
        let mut from_unicode = HashMap::new();
        for c in 0..256usize {
            widths[c] = f64::from(b.widths[c]);
            // Codes are shown as themselves; Unicode is the code point of the same value.
            to_unicode[c] = c as u32;
            from_unicode.insert(c as u32, c as u8);
        }
        FieldFont {
            res_name: res_name.to_string(),
            obj: None,
            ascent: f64::from(b.bbox[3]),
            descent: f64::from(b.bbox[1]),
            cap_height: f64::from(b.bbox[3]),
            imp: FontImpl::Simple(Box::new(SimpleFont {
                to_unicode,
                from_unicode,
                widths,
            })),
            core14: Some(ZAPF_DINGBATS),
        }
    }

    pub fn session(&self) -> Session<'_> {
        match &self.imp {
            FontImpl::Simple(s) => Session::Simple(s),
            FontImpl::Cid(c) => match rustybuzz::Face::from_slice(&c.program, 0) {
                Some(face) => Session::Cid {
                    font: c,
                    face: Box::new(face),
                },
                None => Session::Unusable,
            },
        }
    }

    /// Whether every character of `text` has a glyph.
    pub fn can_show(&self, text: &str) -> bool {
        self.session().run(text).is_some()
    }
}

pub enum Session<'a> {
    Simple(&'a SimpleFont),
    Cid {
        font: &'a CidFont,
        face: Box<rustybuzz::Face<'a>>,
    },
    Unusable,
}

/// A character that is shown as a placeholder when the font cannot display it.
const PLACEHOLDER: char = '?';

impl Session<'_> {
    /// Encode and measure `text`; `None` if any character has no glyph in the font.
    pub fn run(&self, text: &str) -> Option<Run> {
        match self {
            Session::Simple(s) => {
                let mut bytes = Vec::with_capacity(text.len());
                let mut width = 0.0;
                for ch in text.chars() {
                    let b = s.encode(ch)?;
                    bytes.push(b);
                    width += s.widths[usize::from(b)];
                }
                Some(Run {
                    items: vec![TextItem::Text(bytes)],
                    width,
                })
            }
            Session::Cid { font, face } => font.run(face, text),
            Session::Unusable => None,
        }
    }

    /// Like [`run`](Self::run), but characters without a glyph become `?`. The flag says whether
    /// anything was replaced.
    pub fn run_lossy(&self, text: &str) -> (Run, bool) {
        if let Some(r) = self.run(text) {
            return (r, false);
        }
        let fixed: String = text
            .chars()
            .map(|c| {
                if self.run(c.encode_utf8(&mut [0; 4])).is_some() {
                    c
                } else {
                    PLACEHOLDER
                }
            })
            .collect();
        let r = self.run(&fixed).unwrap_or(Run {
            items: Vec::new(),
            width: 0.0,
        });
        (r, true)
    }

    /// Advance of `text` in 1/1000 em (placeholders for missing glyphs).
    pub fn measure(&self, text: &str) -> f64 {
        self.run_lossy(text).0.width
    }
}

impl SimpleFont {
    fn encode(&self, ch: char) -> Option<u8> {
        let ch = match ch {
            '\u{a0}' | '\t' => ' ',
            c => c,
        };
        self.from_unicode.get(&(ch as u32)).copied()
    }
}

impl CidFont {
    fn width_of(&self, cid: u16) -> f64 {
        self.w.get(&cid).copied().unwrap_or(self.dw)
    }

    fn run(&self, face: &rustybuzz::Face<'_>, text: &str) -> Option<Run> {
        if text.is_empty() {
            return Some(Run {
                items: Vec::new(),
                width: 0.0,
            });
        }
        let mut buf = rustybuzz::UnicodeBuffer::new();
        let text: String = text
            .chars()
            .map(|c| if c == '\u{a0}' || c == '\t' { ' ' } else { c })
            .collect();
        buf.push_str(&text);
        buf.guess_segment_properties();
        let out = rustybuzz::shape(face, &[], buf);
        let upem = f64::from(face.units_per_em().max(1));
        let mut items: Vec<TextItem> = Vec::new();
        let mut cur: Vec<u8> = Vec::new();
        let mut width = 0.0;
        for (info, pos) in out.glyph_infos().iter().zip(out.glyph_positions()) {
            let gid = u16::try_from(info.glyph_id).ok()?;
            if gid == 0 {
                return None;
            }
            let cid = match &self.gid_to_cid {
                None => gid,
                Some(m) => *m.get(&gid)?,
            };
            let adv = f64::from(pos.x_advance) * 1000.0 / upem;
            let w = self.width_of(cid);
            cur.extend_from_slice(&cid.to_be_bytes());
            width += adv;
            // Make the PDF advance (w) equal the shaped advance.
            if (w - adv).abs() > 0.5 {
                items.push(TextItem::Text(std::mem::take(&mut cur)));
                items.push(TextItem::Adjust(w - adv));
            }
        }
        if !cur.is_empty() {
            items.push(TextItem::Text(cur));
        }
        Some(Run { items, width })
    }
}

fn simple_from_base14(id: Base14Id) -> SimpleFont {
    let b = base14(id);
    let mut f = SimpleFont {
        to_unicode: base14_data::WIN_ANSI.map(u32::from),
        from_unicode: HashMap::new(),
        widths: [0.0; 256],
    };
    for c in 32..256usize {
        f.widths[c] = f64::from(b.widths[c]);
        let u = f.to_unicode[c];
        if u != 0 {
            f.from_unicode.entry(u).or_insert(c as u8);
        }
    }
    f
}

fn glyph_unicode(name: &str) -> Option<u32> {
    if let Ok(i) = GLYPH_NAMES.binary_search_by(|(n, _)| (*n).cmp(name)) {
        return Some(u32::from(GLYPH_NAMES[i].1));
    }
    if let Some(h) = name.strip_prefix("uni").filter(|h| h.len() == 4)
        && let Ok(v) = u32::from_str_radix(h, 16)
    {
        return Some(v);
    }
    if let Some(h) = name
        .strip_prefix('u')
        .filter(|h| (4..=6).contains(&h.len()))
        && let Ok(v) = u32::from_str_radix(h, 16)
    {
        return Some(v);
    }
    None
}

/// Find the font named `res_name` in the widget's own appearance resources, then in the AcroForm
/// `/DR`. A name that resolves nowhere is a synthetic Helvetica (what Acrobat falls back to).
pub fn resolve(res_name: &str, widget: Option<&Object>, dr: Option<&Object>) -> Result<FieldFont> {
    let lookup = |res: &Object| -> Option<Object> {
        cosx::get(res, "Font").and_then(|f| cosx::get(&f, res_name))
    };
    let from_ap = widget
        .and_then(|w| cosx::get(w, "AP"))
        .and_then(|ap| cosx::get(&ap, "N"))
        .filter(|n| cosx::kind(n) == Some(ObjectKind::Stream))
        .and_then(|n| cosx::get(&n, "Resources"))
        .and_then(|r| lookup(&r));
    let found = from_ap.or_else(|| dr.and_then(lookup));
    match found {
        Some(font) => from_font_object(res_name, &font),
        None => Ok(FieldFont::synthetic_helvetica(res_name)),
    }
}

pub fn from_font_object(res_name: &str, font: &Object) -> Result<FieldFont> {
    match cosx::get_name(font, "Subtype").as_deref() {
        Some("Type0") => cid_from_object(res_name, font),
        Some("Type3") => Err(FillError::Appearance(
            "Type 3 fonts cannot be used for form text".into(),
        )),
        _ => Ok(simple_from_object(res_name, font)),
    }
}

fn simple_from_object(res_name: &str, font: &Object) -> FieldFont {
    let base_font = cosx::get_name(font, "BaseFont").unwrap_or_default();
    let desc = cosx::get(font, "FontDescriptor");
    let flags = desc
        .as_ref()
        .and_then(|d| cosx::get_int(d, "Flags"))
        .unwrap_or(0);
    let embedded = desc.as_ref().is_some_and(|d| {
        cosx::get(d, "FontFile").is_some()
            || cosx::get(d, "FontFile2").is_some()
            || cosx::get(d, "FontFile3").is_some()
    });
    let core = base14_for_name(&base_font);
    let b = base14(core);
    let is_truetype = cosx::get_name(font, "Subtype").as_deref() == Some("TrueType");
    let symbolic = flags & 4 != 0 && flags & 32 == 0 || b.symbolic;

    // Encoding: code -> Unicode.
    let enc = cosx::get(font, "Encoding");
    let (base_name, differences) = match &enc {
        Some(e) if cosx::kind(e) == Some(ObjectKind::Name) => (cosx::name(e), None),
        Some(e) if cosx::is_dict(e) => (
            cosx::get_name(e, "BaseEncoding"),
            cosx::get(e, "Differences"),
        ),
        _ => (None, None),
    };
    let mut to_unicode: [u32; 256] = match base_name.as_deref() {
        Some("WinAnsiEncoding") => base14_data::WIN_ANSI.map(u32::from),
        Some("MacRomanEncoding") => base14_data::MAC_ROMAN.map(u32::from),
        Some("StandardEncoding") => base14_data::STANDARD.map(u32::from),
        _ if b.symbolic => {
            // Built-in encoding of Symbol / ZapfDingbats: codes stand for themselves.
            let mut t = [0u32; 256];
            for (i, v) in t.iter_mut().enumerate() {
                *v = i as u32;
            }
            t
        }
        _ if is_truetype || symbolic || embedded => base14_data::WIN_ANSI.map(u32::from),
        _ => base14_data::STANDARD.map(u32::from),
    };
    if let Some(d) = differences {
        let mut code = 0usize;
        for it in cosx::items(&d) {
            if let Some(n) = cosx::int(&it) {
                code = n.clamp(0, 255) as usize;
            } else if let Some(name) = cosx::name(&it) {
                if code < 256 {
                    to_unicode[code] = glyph_unicode(&name).unwrap_or(0);
                }
                code += 1;
            }
        }
    }

    // Widths: /Widths first, then the Core 14 metrics by character.
    let first = cosx::get_int(font, "FirstChar").unwrap_or(0);
    let widths_arr = cosx::get(font, "Widths").map(|w| cosx::numbers(&w));
    let missing = desc
        .as_ref()
        .and_then(|d| cosx::get_num(d, "MissingWidth"))
        .unwrap_or(0.0);
    let mut widths = [0.0f64; 256];
    for (code, w) in widths.iter_mut().enumerate() {
        let from_array = widths_arr.as_ref().and_then(|a| {
            let i = code as i64 - first;
            usize::try_from(i).ok().and_then(|i| a.get(i).copied())
        });
        *w = from_array.unwrap_or_else(|| {
            if widths_arr.is_some() {
                return missing;
            }
            let u = to_unicode[code];
            if b.symbolic {
                f64::from(b.widths[code])
            } else {
                winansi_byte(u).map_or(0.0, |wb| f64::from(b.widths[usize::from(wb)]))
            }
        });
    }
    let mut from_unicode = HashMap::new();
    for code in 0..256usize {
        let u = to_unicode[code];
        if u != 0 && (code >= 32 || widths[code] > 0.0) {
            from_unicode.entry(u).or_insert(code as u8);
        }
    }
    // Space always encodes if the font has any width for it.
    let ascent = desc
        .as_ref()
        .and_then(|d| cosx::get_num(d, "Ascent"))
        .filter(|a| *a > 0.0)
        .unwrap_or(f64::from(b.ascent));
    let descent = desc
        .as_ref()
        .and_then(|d| cosx::get_num(d, "Descent"))
        .filter(|d| *d < 0.0)
        .unwrap_or(f64::from(b.descent));
    let cap = desc
        .as_ref()
        .and_then(|d| cosx::get_num(d, "CapHeight"))
        .filter(|c| *c > 0.0)
        .unwrap_or(f64::from(b.cap_height));
    FieldFont {
        res_name: res_name.to_string(),
        obj: font.is_indirect().then(|| font.clone()),
        ascent,
        descent,
        cap_height: cap,
        imp: FontImpl::Simple(Box::new(SimpleFont {
            to_unicode,
            from_unicode,
            widths,
        })),
        core14: Some(core),
    }
}

fn cid_from_object(res_name: &str, font: &Object) -> Result<FieldFont> {
    let unsupported = |why: &str| FillError::Appearance(format!("font `{res_name}`: {why}"));
    match cosx::get(font, "Encoding") {
        Some(e) if cosx::name(&e).as_deref() == Some("Identity-H") => {}
        _ => return Err(unsupported("only Identity-H Type0 fonts are supported")),
    }
    let desc_font = cosx::get(font, "DescendantFonts")
        .and_then(|d| cosx::items(&d).into_iter().next())
        .ok_or_else(|| unsupported("no descendant font"))?;
    let descriptor =
        cosx::get(&desc_font, "FontDescriptor").ok_or_else(|| unsupported("no font descriptor"))?;
    let prog = cosx::get(&descriptor, "FontFile2")
        .or_else(|| cosx::get(&descriptor, "FontFile3"))
        .ok_or_else(|| unsupported("no embedded TrueType/OpenType program"))?;
    let program = prog
        .stream_decoded(DecodeLevel::Generalized)
        .map_err(FillError::Cos)?
        .to_vec();
    let face = rustybuzz::Face::from_slice(&program, 0)
        .ok_or_else(|| unsupported("the font program is not a usable TrueType/OpenType font"))?;
    let upem = f64::from(face.units_per_em().max(1));
    let ascent = cosx::get_num(&descriptor, "Ascent")
        .filter(|a| *a > 0.0)
        .unwrap_or_else(|| f64::from(face.ascender()) * 1000.0 / upem);
    let descent = cosx::get_num(&descriptor, "Descent")
        .filter(|d| *d < 0.0)
        .unwrap_or_else(|| f64::from(face.descender()) * 1000.0 / upem);
    let cap = cosx::get_num(&descriptor, "CapHeight")
        .filter(|c| *c > 0.0)
        .unwrap_or(ascent * 0.7);
    drop(face);

    let dw = cosx::get_num(&desc_font, "DW").unwrap_or(1000.0);
    let mut w = HashMap::new();
    if let Some(arr) = cosx::get(&desc_font, "W") {
        let it = cosx::items(&arr);
        let mut i = 0;
        while i < it.len() {
            let Some(c0) = cosx::int(&it[i]) else { break };
            match it.get(i + 1) {
                Some(next) if cosx::kind(next) == Some(ObjectKind::Array) => {
                    for (k, wv) in cosx::numbers(next).into_iter().enumerate() {
                        if let Ok(c) = u16::try_from(c0 + k as i64) {
                            w.insert(c, wv);
                        }
                    }
                    i += 2;
                }
                Some(next) => {
                    let (Some(c1), Some(wv)) = (cosx::int(next), it.get(i + 2).and_then(cosx::num))
                    else {
                        break;
                    };
                    for c in c0..=c1.min(c0 + 65535) {
                        if let Ok(c) = u16::try_from(c) {
                            w.insert(c, wv);
                        }
                    }
                    i += 3;
                }
                None => break,
            }
        }
    }
    let gid_to_cid = match cosx::get(&desc_font, "CIDToGIDMap") {
        Some(m) if cosx::kind(&m) == Some(ObjectKind::Stream) => {
            let data = m
                .stream_decoded(DecodeLevel::Generalized)
                .map_err(FillError::Cos)?;
            let mut inv = HashMap::new();
            for (cid, pair) in data.as_slice().as_chunks::<2>().0.iter().enumerate() {
                let gid = u16::from_be_bytes(*pair);
                if gid != 0 && cid <= 0xFFFF {
                    inv.entry(gid).or_insert(cid as u16);
                }
            }
            Some(inv)
        }
        _ => None,
    };
    Ok(FieldFont {
        res_name: res_name.to_string(),
        obj: font.is_indirect().then(|| font.clone()),
        ascent,
        descent,
        cap_height: cap,
        imp: FontImpl::Cid(Box::new(CidFont {
            program,
            dw,
            w,
            gid_to_cid,
        })),
        core14: None,
    })
}

/// Create (once) the indirect Helvetica font object for a synthetic font and register it under
/// `res_name` in the AcroForm `/DR`, as Acrobat does. Returns the object to reference.
pub fn ensure_helvetica_object(doc: &Document) -> Result<Object> {
    let d = doc.new_dict();
    d.dict_set("Type", &doc.new_name("Font")?)?;
    d.dict_set("Subtype", &doc.new_name("Type1")?)?;
    d.dict_set("BaseFont", &doc.new_name("Helvetica")?)?;
    d.dict_set("Encoding", &doc.new_name("WinAnsiEncoding")?)?;
    Ok(doc.make_indirect(&d)?)
}

/// A direct (unnumbered) ZapfDingbats font dictionary for appearance resources.
pub fn zapf_font_dict(doc: &Document) -> Result<Object> {
    let d = doc.new_dict();
    d.dict_set("Type", &doc.new_name("Font")?)?;
    d.dict_set("Subtype", &doc.new_name("Type1")?)?;
    d.dict_set("BaseFont", &doc.new_name("ZapfDingbats")?)?;
    Ok(d)
}
