//! Writing annotation dictionaries and appearance streams into a document, and reading a
//! [`Spec`] back from one.

use papyrine_cos::{Document, ObjId, Object, ObjectKind};
use papyrine_ops::{EditContext, Error, Result, decode_text_string, encode_text_string};

use crate::appearance::{Appearance, default_appearance, default_style_string, rich_content};
use crate::geometry::{
    Align, FontFamily, Geometry, LineEnding, MarkupKind, Pt, Quad, TextStyle, normalize_rect,
};
use crate::props::{AnnotProps, Color, flags};
use crate::spec::Spec;
use crate::text::embed;

pub fn fmt(x: f64) -> String {
    let s = format!("{x:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-0" {
        "0".into()
    } else {
        s.to_owned()
    }
}

pub fn num_array(doc: &Document, v: &[f64]) -> Result<Object> {
    let body: Vec<String> = v.iter().map(|x| fmt(*x)).collect();
    Ok(doc.parse_object(format!("[{}]", body.join(" ")))?)
}

fn text_string(doc: &Document, s: &str) -> Result<Object> {
    Ok(doc.new_string(encode_text_string(s))?)
}

/// `dict[key] = value`, or removes the key when `value` is `None`.
fn put(doc: &Document, dict: &Object, key: &str, value: Option<Object>) -> Result<()> {
    match value {
        Some(v) => dict.dict_set(key, &v)?,
        None => dict.dict_remove(key)?,
    }
    let _ = doc;
    Ok(())
}

/// Dates and name chosen when the command was built.
pub struct Fixed<'a> {
    pub page: Option<&'a Object>,
    /// Extra keys for replies: `/IRT` target.
    pub irt: Option<&'a Object>,
}

/// Fill `annot` (an indirect dictionary, already touched when it pre-exists) from `spec`/`app`.
pub fn fill_annotation(
    doc: &Document,
    annot: &Object,
    spec: &Spec,
    app: &Appearance,
    fixed: &Fixed<'_>,
) -> Result<()> {
    let p = &spec.props;
    annot.dict_set("Type", &doc.new_name("Annot")?)?;
    annot.dict_set("Subtype", &doc.new_name(spec.geometry.subtype())?)?;
    if let Some(page) = fixed.page {
        annot.dict_set("P", page)?;
    }
    annot.dict_set("Rect", &num_array(doc, &app.rect)?)?;
    let default_flags = match spec.geometry {
        Geometry::Note { .. } => flags::NOTE_DEFAULT,
        _ => flags::PRINT,
    };
    annot.dict_set(
        "F",
        &doc.new_int(i64::from(p.flags.unwrap_or(default_flags))),
    )?;
    annot.dict_set("C", &num_array(doc, &spec.color().0)?)?;
    let ca = spec.opacity();
    put(
        doc,
        annot,
        "CA",
        ((ca - 1.0).abs() > 1e-9)
            .then(|| doc.parse_object(fmt(ca)))
            .transpose()?,
    )?;
    put(
        doc,
        annot,
        "T",
        p.author
            .as_deref()
            .map(|s| text_string(doc, s))
            .transpose()?,
    )?;
    put(
        doc,
        annot,
        "Subj",
        p.subject
            .as_deref()
            .map(|s| text_string(doc, s))
            .transpose()?,
    )?;
    put(
        doc,
        annot,
        "Contents",
        Some(text_string(doc, p.contents.as_deref().unwrap_or(""))?),
    )?;
    if let Some(m) = &p.modified {
        annot.dict_set("M", &doc.new_string(m.as_bytes())?)?;
    }
    if let Some(m) = &p.created {
        annot.dict_set("CreationDate", &doc.new_string(m.as_bytes())?)?;
    }
    if let Some(n) = &p.name {
        annot.dict_set("NM", &text_string(doc, n)?)?;
    }
    put(
        doc,
        annot,
        "IC",
        p.fill.as_ref().map(|c| num_array(doc, &c.0)).transpose()?,
    )?;
    if let Some(irt) = fixed.irt {
        annot.dict_set("IRT", irt)?;
        annot.dict_set("RT", &doc.new_name("R")?)?;
    }

    // Border style (shapes, lines, ink, underline-type markup, text box border).
    let w = spec.width();
    let wants_bs = !matches!(spec.geometry, Geometry::Note { .. })
        && !matches!(
            spec.geometry,
            Geometry::TextMarkup {
                kind: MarkupKind::Highlight,
                ..
            }
        );
    if wants_bs {
        let dash = if p.dash.is_empty() {
            String::new()
        } else {
            format!(
                " /D [{}]",
                p.dash.iter().map(|d| fmt(*d)).collect::<Vec<_>>().join(" ")
            )
        };
        let s = if p.dash.is_empty() { "S" } else { "D" };
        annot.dict_set(
            "BS",
            &doc.parse_object(format!("<< /Type /Border /W {} /S /{s}{dash} >>", fmt(w)))?,
        )?;
    } else {
        annot.dict_remove("BS")?;
    }
    put(
        doc,
        annot,
        "RD",
        app.rd.map(|r| num_array(doc, &r)).transpose()?,
    )?;

    match &spec.geometry {
        Geometry::TextMarkup { quads, .. } => {
            let flat: Vec<f64> = quads.iter().flat_map(|q| q.flatten()).collect();
            annot.dict_set("QuadPoints", &num_array(doc, &flat)?)?;
        }
        Geometry::Note { icon, .. } => {
            annot.dict_set("Name", &doc.new_name(icon)?)?;
            annot.dict_set("Open", &doc.new_bool(false))?;
        }
        Geometry::TextBox { style, .. } => {
            let text = p.contents.clone().unwrap_or_default();
            annot.dict_set("DA", &text_string(doc, &default_appearance(style))?)?;
            annot.dict_set("DS", &text_string(doc, &default_style_string(style))?)?;
            annot.dict_set("Q", &doc.new_int(style.align.q()))?;
            annot.dict_set("RC", &text_string(doc, &rich_content(&text, style))?)?;
        }
        Geometry::Ink { strokes } => {
            let list = doc.new_array();
            for s in strokes {
                let flat: Vec<f64> = s.iter().flat_map(|p| [p[0], p[1]]).collect();
                list.array_push(&num_array(doc, &flat)?)?;
            }
            annot.dict_set("InkList", &list)?;
        }
        Geometry::Square { .. } | Geometry::Circle { .. } => {}
        Geometry::Line {
            from,
            to,
            start,
            end,
        } => {
            annot.dict_set("L", &num_array(doc, &[from[0], from[1], to[0], to[1]])?)?;
            annot.dict_set(
                "LE",
                &doc.parse_object(format!("[/{} /{}]", start.name(), end.name()))?,
            )?;
            if spec.geometry.is_arrow() {
                annot.dict_set("IT", &doc.new_name("LineArrow")?)?;
            } else {
                annot.dict_remove("IT")?;
            }
        }
    }
    Ok(())
}

/// Write `/AP /N` for `annot`, reusing an existing indirect form stream when there is one.
pub fn write_appearance(cx: &mut EditContext<'_>, annot: &Object, app: &Appearance) -> Result<()> {
    let doc = cx.doc();
    let existing = annot
        .dict_get("AP")
        .and_then(|ap| {
            if ap.kind()? == ObjectKind::Dictionary {
                ap.dict_get("N")
            } else {
                Ok(ap)
            }
        })
        .ok()
        .filter(|n| n.is_indirect() && n.kind().ok() == Some(ObjectKind::Stream));
    let stream = match existing {
        Some(s) => {
            cx.touch(&s)?;
            let d = s.stream_dict()?;
            for k in d.dict_keys()? {
                if !matches!(k.as_slice(), b"Length" | b"Filter" | b"DecodeParms") {
                    d.dict_remove(&k)?;
                }
            }
            s
        }
        None => doc.new_stream(b"")?,
    };
    // Resources.
    let res = doc.new_dict();
    if app.opacity.is_some() || app.multiply {
        let ca = app.opacity.unwrap_or(1.0);
        let bm = if app.multiply { " /BM /Multiply" } else { "" };
        let gs = doc.parse_object(format!(
            "<< /GS0 << /Type /ExtGState /CA {c} /ca {c} /AIS false{bm} >> >>",
            c = fmt(ca)
        ))?;
        res.dict_set("ExtGState", &gs)?;
    }
    if !app.fonts.is_empty() {
        let fd = doc.new_dict();
        for f in &app.fonts {
            fd.dict_set(&f.resource, &embed::materialize(doc, f)?)?;
        }
        res.dict_set("Font", &fd)?;
    }
    let dict = stream.stream_dict()?;
    dict.dict_set("Type", &doc.new_name("XObject")?)?;
    dict.dict_set("Subtype", &doc.new_name("Form")?)?;
    dict.dict_set("FormType", &doc.new_int(1))?;
    dict.dict_set("BBox", &num_array(doc, &app.bbox)?)?;
    dict.dict_set("Resources", &res)?;
    if app.content.len() > 512 {
        let z = embed::deflate(&app.content);
        stream.stream_replace(&z, Some(&doc.new_name("FlateDecode")?), None)?;
    } else {
        stream.stream_replace(&app.content, None, None)?;
    }
    let ap = doc.new_dict();
    ap.dict_set("N", &stream)?;
    annot.dict_set("AP", &ap)?;
    Ok(())
}

// --- reading -------------------------------------------------------------------------------

fn nums(o: &Object) -> Vec<f64> {
    if o.kind().ok() != Some(ObjectKind::Array) {
        return Vec::new();
    }
    o.array_items()
        .map(|v| v.iter().filter_map(|x| x.as_f64().ok()).collect())
        .unwrap_or_default()
}

fn get_nums(a: &Object, key: &str) -> Vec<f64> {
    a.dict_get(key).map(|o| nums(&o)).unwrap_or_default()
}

fn get_text(a: &Object, key: &str) -> Option<String> {
    let v = a.dict_get(key).ok()?;
    (v.kind().ok()? == ObjectKind::String)
        .then(|| v.string().ok().map(|b| decode_text_string(&b)))
        .flatten()
}

fn get_name(a: &Object, key: &str) -> Option<String> {
    let v = a.dict_get(key).ok()?;
    (v.kind().ok()? == ObjectKind::Name)
        .then(|| {
            v.name()
                .ok()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
        })
        .flatten()
}

fn pt_pairs(v: &[f64]) -> Vec<Pt> {
    v.as_chunks::<2>().0.iter().map(|c| [c[0], c[1]]).collect()
}

/// Parse a `/DA` string into a text style (font alias, size, colour).
pub fn parse_da(da: &str) -> TextStyle {
    let mut st = TextStyle::default();
    let toks: Vec<&str> = da.split_whitespace().collect();
    let mut i = 0;
    while i < toks.len() {
        match toks[i] {
            "Tf" if i >= 2 => {
                if let Ok(sz) = toks[i - 1].parse::<f64>() {
                    st.size = sz.clamp(1.0, 500.0);
                }
                let f = toks[i - 2].trim_start_matches('/');
                let lower = f.to_lowercase();
                st.bold = f.ends_with("Bo") || lower.contains("bold");
                st.family = if lower.starts_with("ti")
                    || lower.contains("times")
                    || lower.contains("serif") && !lower.contains("sans")
                {
                    FontFamily::Serif
                } else if lower.starts_with("co")
                    || lower.contains("cour")
                    || lower.contains("mono")
                {
                    FontFamily::Mono
                } else {
                    FontFamily::Sans
                };
            }
            "g" if i >= 1 => {
                if let Ok(v) = toks[i - 1].parse::<f64>() {
                    st.color = Color::gray(v.clamp(0.0, 1.0));
                }
            }
            "rg" if i >= 3 => {
                let p: Vec<f64> = toks[i - 3..i]
                    .iter()
                    .filter_map(|t| t.parse().ok())
                    .collect();
                if p.len() == 3 {
                    st.color = Color::rgb(
                        p[0].clamp(0.0, 1.0),
                        p[1].clamp(0.0, 1.0),
                        p[2].clamp(0.0, 1.0),
                    );
                }
            }
            "k" if i >= 4 => {
                let p: Vec<f64> = toks[i - 4..i]
                    .iter()
                    .filter_map(|t| t.parse().ok())
                    .collect();
                if p.len() == 4 {
                    st.color = Color(p.iter().map(|v| v.clamp(0.0, 1.0)).collect());
                }
            }
            _ => {}
        }
        i += 1;
    }
    st
}

/// Read an annotation of a type Papyrine can regenerate. `None` for other types.
pub fn read_spec(a: &Object) -> Option<Spec> {
    if a.kind().ok()? != ObjectKind::Dictionary {
        return None;
    }
    let subtype = get_name(a, "Subtype")?;
    let rect = {
        let r = get_nums(a, "Rect");
        (r.len() == 4).then(|| normalize_rect([r[0], r[1], r[2], r[3]]))?
    };
    let geometry = match subtype.as_str() {
        "Highlight" | "Underline" | "StrikeOut" | "Squiggly" => {
            let kind = match subtype.as_str() {
                "Highlight" => MarkupKind::Highlight,
                "Underline" => MarkupKind::Underline,
                "StrikeOut" => MarkupKind::StrikeOut,
                _ => MarkupKind::Squiggly,
            };
            let q = get_nums(a, "QuadPoints");
            let quads: Vec<Quad> = q
                .as_chunks::<8>()
                .0
                .iter()
                .map(|c| Quad([[c[0], c[1]], [c[2], c[3]], [c[4], c[5]], [c[6], c[7]]]))
                .collect();
            if quads.is_empty() {
                return None;
            }
            Geometry::TextMarkup { kind, quads }
        }
        "Text" => Geometry::Note {
            pos: [rect[0], rect[3]],
            icon: get_name(a, "Name").unwrap_or_else(|| "Note".into()),
        },
        "FreeText" => {
            let mut style = parse_da(&get_text(a, "DA").unwrap_or_default());
            style.align = Align::from_q(
                a.dict_get("Q")
                    .ok()
                    .and_then(|q| q.as_int().ok())
                    .unwrap_or(0),
            );
            Geometry::TextBox { rect, style }
        }
        "Ink" => {
            let list = a.dict_get("InkList").ok()?;
            let strokes: Vec<Vec<Pt>> = list
                .array_items()
                .ok()?
                .iter()
                .map(|s| pt_pairs(&nums(s)))
                .filter(|s| !s.is_empty())
                .collect();
            if strokes.is_empty() {
                return None;
            }
            Geometry::Ink { strokes }
        }
        "Square" => Geometry::Square { rect },
        "Circle" => Geometry::Circle { rect },
        "Line" => {
            let l = get_nums(a, "L");
            if l.len() < 4 {
                return None;
            }
            let le: Vec<LineEnding> = a
                .dict_get("LE")
                .ok()
                .and_then(|o| o.array_items().ok())
                .map(|v| {
                    v.iter()
                        .map(|n| {
                            n.name()
                                .map(|b| LineEnding::from_name(&String::from_utf8_lossy(&b)))
                                .unwrap_or_default()
                        })
                        .collect()
                })
                .unwrap_or_default();
            Geometry::Line {
                from: [l[0], l[1]],
                to: [l[2], l[3]],
                start: le.first().copied().unwrap_or_default(),
                end: le.get(1).copied().unwrap_or_default(),
            }
        }
        _ => return None,
    };
    let color = Color::from_components(get_nums(a, "C")).filter(|c| !c.0.is_empty());
    let fill = Color::from_components(get_nums(a, "IC")).filter(|c| !c.0.is_empty());
    let bs = a
        .dict_get("BS")
        .ok()
        .filter(|b| b.kind().ok() == Some(ObjectKind::Dictionary));
    let width = bs
        .as_ref()
        .and_then(|b| b.dict_get("W").ok())
        .and_then(|w| w.as_f64().ok())
        .or_else(|| {
            let b = get_nums(a, "Border");
            (b.len() >= 3).then(|| b[2])
        });
    let dash = bs
        .as_ref()
        .filter(|b| get_name(b, "S").as_deref() == Some("D"))
        .map(|b| get_nums(b, "D"))
        .unwrap_or_default();
    let opacity = a.dict_get("CA").ok().and_then(|c| c.as_f64().ok());
    let props = AnnotProps {
        color,
        opacity,
        width,
        dash,
        fill,
        author: get_text(a, "T"),
        subject: get_text(a, "Subj"),
        contents: get_text(a, "Contents"),
        created: get_text(a, "CreationDate"),
        modified: get_text(a, "M"),
        flags: a
            .dict_get("F")
            .ok()
            .and_then(|f| f.as_int().ok())
            .map(|f| f as u32),
        name: get_text(a, "NM"),
    };
    Some(Spec { geometry, props })
}

/// Find the annotation `target` on `page`: the `/Annots` index and the object.
pub fn find_in_page(page: &Object, target: &crate::commands::AnnotRef) -> Result<(usize, Object)> {
    let annots = page.dict_get("Annots")?;
    if annots.kind()? != ObjectKind::Array {
        return Err(Error::invalid("the page has no annotations"));
    }
    for (i, a) in annots.array_items()?.into_iter().enumerate() {
        let hit = match target {
            crate::commands::AnnotRef::Id { num, generation } => {
                a.id() == Some(ObjId::new(*num, *generation))
            }
            crate::commands::AnnotRef::Name(n) => get_text(&a, "NM").as_deref() == Some(n.as_str()),
            crate::commands::AnnotRef::Index(n) => i == *n,
        };
        if hit {
            return Ok((i, a));
        }
    }
    Err(Error::invalid(format!(
        "annotation {target:?} not found on the page"
    )))
}

/// `/Rect` of an annotation dictionary.
pub fn read_rect(a: &Object) -> Option<[f64; 4]> {
    let r = get_nums(a, "Rect");
    (r.len() == 4).then(|| [r[0], r[1], r[2], r[3]])
}
