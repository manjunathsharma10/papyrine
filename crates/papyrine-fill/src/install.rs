//! Writing generated appearances into the document, always through the `EditContext` so that
//! every changed object has a before-image.

use papyrine_cos::{Object, ObjectKind};
use papyrine_ops::EditContext;

use crate::appearance::{Ap, FontUse};
use crate::cosx;
use crate::error::{FillError, Result};
use crate::font;
use crate::form::Widget;

/// Marker key on streams this crate generated and owns exclusively, so that later edits can
/// rewrite them in place instead of leaving orphans behind.
pub(crate) const MARKER: &str = "PapyrineFill";

fn real_array(cx: &EditContext<'_>, v: &[f64]) -> Result<Object> {
    let a = cx.doc().new_array();
    for &x in v {
        a.array_push(&cx.doc().new_real(x)?)?;
    }
    Ok(a)
}

/// Record the object that physically stores the AcroForm dictionary: itself when indirect,
/// else the catalog it is embedded in.
pub(crate) fn touch_acro(cx: &mut EditContext<'_>, acro: &Object) -> Result<()> {
    if acro.is_indirect() {
        cx.touch(acro)?;
    } else {
        let root = cx.doc().root()?;
        cx.touch(&root)?;
    }
    Ok(())
}

/// Record the holder of something stored inside `/DR`.
fn touch_dr(cx: &mut EditContext<'_>, acro: &Object, dr: &Object) -> Result<()> {
    if dr.is_indirect() {
        cx.touch(dr)?;
        Ok(())
    } else {
        touch_acro(cx, acro)
    }
}

/// Find or create the indirect font object for a synthetic Helvetica named `name` and make sure
/// the AcroForm `/DR /Font` lists it (as Acrobat does when it fills a field whose font is missing).
pub(crate) fn synthetic_font(
    cx: &mut EditContext<'_>,
    acro: Option<&Object>,
    name: &str,
) -> Result<Object> {
    let doc = cx.doc();
    let Some(acro) = acro else {
        return font::ensure_helvetica_object(doc);
    };
    let dr = match cosx::get(acro, "DR").filter(cosx::is_dict) {
        Some(d) => d,
        None => {
            touch_acro(cx, acro)?;
            let d = doc.new_dict();
            acro.dict_set("DR", &d)?;
            d
        }
    };
    let fonts = match cosx::get(&dr, "Font").filter(cosx::is_dict) {
        Some(f) => f,
        None => {
            touch_dr(cx, acro, &dr)?;
            let f = doc.new_dict();
            dr.dict_set("Font", &f)?;
            f
        }
    };
    if let Some(existing) = cosx::get(&fonts, name)
        && existing.is_indirect()
    {
        return Ok(existing);
    }
    let obj = font::ensure_helvetica_object(doc)?;
    if fonts.is_indirect() {
        cx.touch(&fonts)?;
    } else {
        touch_dr(cx, acro, &dr)?;
    }
    fonts.dict_set(name, &obj)?;
    Ok(obj)
}

/// Record the object that physically stores `dict` (itself when indirect, else `owner`).
fn touch_holder(cx: &mut EditContext<'_>, dict: &Object, owner: &Object) -> Result<()> {
    if dict.is_indirect() {
        cx.touch(dict)?;
    } else {
        cx.touch(owner)?;
    }
    Ok(())
}

fn resources(
    cx: &mut EditContext<'_>,
    acro: Option<&Object>,
    font: Option<&FontUse>,
) -> Result<Object> {
    let doc = cx.doc();
    let res = doc.new_dict();
    if let Some(fu) = font {
        let fobj = if fu.zapf {
            font::zapf_font_dict(doc)?
        } else if let Some(o) = &fu.obj {
            o.clone()
        } else {
            synthetic_font(cx, acro, &fu.name)?
        };
        let fonts = doc.new_dict();
        fonts.dict_set(&fu.name, &fobj)?;
        res.dict_set("Font", &fonts)?;
    }
    Ok(res)
}

/// Create (or rewrite in place) the form XObject for `ap`.
fn write_xobject(
    cx: &mut EditContext<'_>,
    acro: Option<&Object>,
    ap: &Ap,
    reuse: Option<Object>,
) -> Result<Object> {
    let res = resources(cx, acro, ap.font.as_ref())?;
    let stream = match reuse {
        Some(s) => {
            cx.touch(&s)?;
            s
        }
        None => {
            let s = cx.doc().new_stream(&[])?;
            if s.is_indirect() {
                s
            } else {
                cx.doc().make_indirect(&s)?
            }
        }
    };
    stream.stream_replace(&ap.content, None, None)?;
    let d = stream.stream_dict()?;
    let doc = cx.doc();
    d.dict_set("Type", &doc.new_name("XObject")?)?;
    d.dict_set("Subtype", &doc.new_name("Form")?)?;
    d.dict_set("FormType", &doc.new_int(1))?;
    d.dict_set("BBox", &real_array(cx, &ap.bbox)?)?;
    match ap.matrix {
        Some(m) => d.dict_set("Matrix", &real_array(cx, &m)?)?,
        None => d.dict_remove("Matrix")?,
    }
    d.dict_set("Resources", &res)?;
    d.dict_set(MARKER, &doc.new_bool(true))?;
    Ok(stream)
}

fn owned(o: &Object) -> bool {
    o.is_indirect()
        && cosx::kind(o) == Some(ObjectKind::Stream)
        && cosx::get_bool(o, MARKER).unwrap_or(false)
}

/// Install `ap` as the widget's normal appearance: `state: None` for a single stream (text,
/// choice), `Some(name)` as one entry of the `/N` state dictionary (check boxes, radios).
pub(crate) fn install(
    cx: &mut EditContext<'_>,
    acro: Option<&Object>,
    widget: &Widget,
    state: Option<&str>,
    ap: &Ap,
) -> Result<()> {
    let w = &widget.obj;
    if !w.is_indirect() {
        return Err(FillError::Appearance("direct widget dictionary".into()));
    }
    // /AP dictionary (create when missing).
    let ap_dict = match cosx::get(w, "AP").filter(cosx::is_dict) {
        Some(a) => a,
        None => {
            cx.touch(w)?;
            let a = cx.doc().new_dict();
            w.dict_set("AP", &a)?;
            a
        }
    };
    touch_holder(cx, &ap_dict, w)?;
    match state {
        None => {
            let reuse = cosx::get(&ap_dict, "N").filter(owned);
            let s = write_xobject(cx, acro, ap, reuse)?;
            ap_dict.dict_set("N", &s)?;
        }
        Some(st) => {
            let n = match cosx::get(&ap_dict, "N")
                .filter(|n| cosx::kind(n) == Some(ObjectKind::Dictionary))
            {
                Some(n) => n,
                None => {
                    let n = cx.doc().new_dict();
                    ap_dict.dict_set("N", &n)?;
                    n
                }
            };
            let n_owner = if ap_dict.is_indirect() { &ap_dict } else { w };
            touch_holder(cx, &n, n_owner)?;
            let reuse = cosx::get(&n, st).filter(owned);
            let s = write_xobject(cx, acro, ap, reuse)?;
            n.dict_set(st, &s)?;
        }
    }
    Ok(())
}

/// A form XObject not tied to a widget (flat-form annotations).
pub(crate) fn standalone_xobject(cx: &mut EditContext<'_>, ap: &Ap) -> Result<Object> {
    write_xobject(cx, None, ap, None)
}
