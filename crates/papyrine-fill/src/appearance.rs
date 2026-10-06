//! Appearance-stream generation for widgets (ISO 32000-2 section 12.7.4.3 and 12.5.5).
//!
//! The generator is ours; qpdf's only handles ASCII text. It supports single-line, multi-line
//! (wrapped), comb, password, quadding, auto font size, combo and list boxes, check boxes and
//! radio buttons (ZapfDingbats marks per `/MK /CA`), borders in all five styles and `/MK /R`
//! rotation. Pure functions: nothing here touches a document.

use papyrine_content::{ContentBuilder, Object as CObj, TextItem, parse};
use papyrine_cos::Object;

use crate::error::{FillError, Result};
use crate::font::{FieldFont, Run, Session, ZAPF_BBOX};
use crate::form::{BorderStyle, Field, Kind, Widget};

/// Default appearance: the font resource name, size (0 = auto) and colour operators.
#[derive(Debug, Clone, PartialEq)]
pub struct Da {
    pub font_name: String,
    pub size: f64,
    pub color: Vec<Op2>,
}

/// A colour operator kept as plain data so `Da` stays `PartialEq`.
#[derive(Debug, Clone, PartialEq)]
pub struct Op2 {
    pub operator: String,
    pub operands: Vec<f64>,
}

impl Default for Da {
    fn default() -> Self {
        Da {
            font_name: "Helv".into(),
            size: 0.0,
            color: vec![Op2 {
                operator: "g".into(),
                operands: vec![0.0],
            }],
        }
    }
}

pub fn parse_da(da: Option<&str>) -> Da {
    let mut out = Da::default();
    let Some(da) = da else { return out };
    let parsed = parse(da.as_bytes());
    let mut color = Vec::new();
    for op in &parsed.ops {
        match op.operator_str().as_ref() {
            "Tf" => {
                if let (Some(n), Some(s)) = (op.operands.first(), op.operands.get(1)) {
                    if let Some(name) = n.as_name() {
                        out.font_name = String::from_utf8_lossy(name).into_owned();
                    }
                    out.size = s.as_f64().unwrap_or(0.0).max(0.0);
                }
            }
            c @ ("g" | "rg" | "k") => {
                color = vec![Op2 {
                    operator: c.to_string(),
                    operands: op.operands.iter().filter_map(CObj::as_f64).collect(),
                }];
            }
            _ => {}
        }
    }
    if !color.is_empty() {
        out.color = color;
    }
    out
}

/// A font resource the stream refers to.
#[derive(Clone)]
pub struct FontUse {
    pub name: String,
    /// `None`: a synthetic Helvetica (installed in `/DR`) or, with `zapf`, a direct ZapfDingbats.
    pub obj: Option<Object>,
    pub zapf: bool,
}

/// A generated form XObject.
#[derive(Clone)]
pub struct Ap {
    pub content: Vec<u8>,
    pub bbox: [f64; 4],
    pub matrix: Option<[f64; 6]>,
    pub font: Option<FontUse>,
    /// Characters the font cannot show were replaced by `?`.
    pub lossy: bool,
}

/// Font sizes tried by auto-sizing, the same ladder PDFium uses.
const AUTO_SIZES: [f64; 29] = [
    4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 14.0, 18.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0,
    50.0, 55.0, 60.0, 70.0, 80.0, 90.0, 100.0, 110.0, 120.0, 130.0, 144.0, 160.0,
];

/// Geometry shared by every generator: box size after `/MK /R`, border, padding.
struct Frame {
    w: f64,
    h: f64,
    bw: f64,
    pad: f64,
    bbox: [f64; 4],
    matrix: Option<[f64; 6]>,
}

impl Frame {
    fn new(widget: &Widget) -> Frame {
        let (rw, rh) = (widget.width().max(0.0), widget.height().max(0.0));
        let (w, h, matrix) = match widget.rotation {
            90 => (rh, rw, Some([0.0, 1.0, -1.0, 0.0, rw, 0.0])),
            180 => (rw, rh, Some([-1.0, 0.0, 0.0, -1.0, rw, rh])),
            270 => (rh, rw, Some([0.0, -1.0, 1.0, 0.0, 0.0, rh])),
            _ => (rw, rh, None),
        };
        let bw = if widget.border_color.is_some() {
            widget.border_width
        } else {
            0.0
        };
        Frame {
            w,
            h,
            bw,
            pad: (2.0f64).max(2.0 * bw),
            bbox: [0.0, 0.0, w, h],
            matrix,
        }
    }
}

fn set_color(b: &mut ContentBuilder, comps: &[f64], fill: bool) {
    let (g, rgb, k) = if fill {
        ("g", "rg", "k")
    } else {
        ("G", "RG", "K")
    };
    let op = match comps.len() {
        1 => g,
        3 => rgb,
        4 => k,
        _ => return,
    };
    b.op(op, comps.iter().map(|&c| CObj::from(c)).collect());
}

fn apply_da_color(b: &mut ContentBuilder, da: &Da, over: Option<[f64; 3]>) {
    if let Some(c) = over {
        set_color(b, &c, true);
        return;
    }
    for op in &da.color {
        b.op(
            &op.operator,
            op.operands.iter().map(|&c| CObj::from(c)).collect(),
        );
    }
}

fn ellipse(b: &mut ContentBuilder, x0: f64, y0: f64, x1: f64, y1: f64) {
    const K: f64 = 0.552_284_749_8;
    let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let (rx, ry) = ((x1 - x0) / 2.0, (y1 - y0) / 2.0);
    b.move_to(cx + rx, cy)
        .curve_to(cx + rx, cy + K * ry, cx + K * rx, cy + ry, cx, cy + ry)
        .curve_to(cx - K * rx, cy + ry, cx - rx, cy + K * ry, cx - rx, cy)
        .curve_to(cx - rx, cy - K * ry, cx - K * rx, cy - ry, cx, cy - ry)
        .curve_to(cx + K * rx, cy - ry, cx + rx, cy - K * ry, cx + rx, cy)
        .close();
}

/// Background and border exactly as `/MK` and `/BS` describe them.
fn draw_frame(b: &mut ContentBuilder, f: &Frame, widget: &Widget, round: bool) {
    let (w, h) = (f.w, f.h);
    if let Some(bg) = widget.background.as_ref().filter(|c| !c.is_empty()) {
        b.save();
        set_color(b, bg, true);
        if round {
            ellipse(b, 0.0, 0.0, w, h);
        } else {
            b.rect(0.0, 0.0, w, h);
        }
        b.fill().restore();
    }
    let Some(bc) = widget.border_color.as_ref().filter(|c| !c.is_empty()) else {
        return;
    };
    let bw = widget.border_width;
    if bw <= 0.0 {
        return;
    }
    b.save();
    match widget.border_style {
        BorderStyle::Solid | BorderStyle::Dashed => {
            set_color(b, bc, false);
            b.line_width(bw);
            if widget.border_style == BorderStyle::Dashed {
                let d = if widget.dash.is_empty() {
                    vec![3.0]
                } else {
                    widget.dash.clone()
                };
                b.dash(&d, 0.0);
            }
            if round {
                ellipse(b, bw / 2.0, bw / 2.0, w - bw / 2.0, h - bw / 2.0);
            } else {
                b.rect(bw / 2.0, bw / 2.0, (w - bw).max(0.0), (h - bw).max(0.0));
            }
            b.stroke();
        }
        BorderStyle::Underline => {
            set_color(b, bc, false);
            b.line_width(bw)
                .move_to(0.0, bw / 2.0)
                .line_to(w, bw / 2.0)
                .stroke();
        }
        BorderStyle::Beveled | BorderStyle::Inset => {
            // Outer frame in the border colour, then two bevel polygons inside it.
            set_color(b, bc, true);
            b.rect(0.0, 0.0, w, h)
                .rect(bw, bw, (w - 2.0 * bw).max(0.0), (h - 2.0 * bw).max(0.0))
                .fill_even_odd();
            let beveled = widget.border_style == BorderStyle::Beveled;
            let light = if beveled { vec![1.0] } else { vec![0.5] };
            let dark = if beveled {
                match widget.background.as_ref() {
                    Some(bg) if !bg.is_empty() => bg.iter().map(|c| c * 0.5).collect(),
                    _ => vec![0.5],
                }
            } else {
                vec![0.75]
            };
            set_color(b, &light, true);
            b.move_to(bw, bw)
                .line_to(bw, h - bw)
                .line_to(w - bw, h - bw)
                .line_to(w - 2.0 * bw, h - 2.0 * bw)
                .line_to(2.0 * bw, h - 2.0 * bw)
                .line_to(2.0 * bw, 2.0 * bw)
                .close()
                .fill();
            set_color(b, &dark, true);
            b.move_to(w - bw, h - bw)
                .line_to(w - bw, bw)
                .line_to(bw, bw)
                .line_to(2.0 * bw, 2.0 * bw)
                .line_to(w - 2.0 * bw, 2.0 * bw)
                .line_to(w - 2.0 * bw, h - 2.0 * bw)
                .close()
                .fill();
        }
    }
    b.restore();
}

fn show(b: &mut ContentBuilder, run: &Run) {
    match run.items.as_slice() {
        [] => {}
        [TextItem::Text(t)] => {
            b.show_text(t);
        }
        items => {
            b.show_text_adjusted(items);
        }
    }
}

fn pts(width_1000: f64, size: f64) -> f64 {
    width_1000 * size / 1000.0
}

/// Break `text` into lines no wider than `avail` points at `size`.
fn wrap(sess: &Session<'_>, text: &str, size: f64, avail: f64) -> Vec<String> {
    let fits = |s: &str| pts(sess.measure(s), size) <= avail + 1e-6;
    // A word wider than the box is cut by characters; the last piece stays open.
    let break_long = |word: &str, lines: &mut Vec<String>| -> String {
        let mut piece = String::new();
        for ch in word.chars() {
            let mut t = piece.clone();
            t.push(ch);
            if !piece.is_empty() && !fits(&t) {
                lines.push(std::mem::take(&mut piece));
            }
            piece.push(ch);
        }
        piece
    };
    let mut lines = Vec::new();
    for para in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        if para.is_empty() {
            lines.push(String::new());
            continue;
        }
        // Words keep their leading space so measuring sees the real advance.
        let mut words: Vec<String> = Vec::new();
        let mut word = String::new();
        for ch in para.chars() {
            if ch == ' ' && !word.is_empty() && !word.ends_with(' ') {
                words.push(std::mem::take(&mut word));
            }
            word.push(ch);
        }
        if !word.is_empty() {
            words.push(word);
        }
        let mut cur = String::new();
        for w in words {
            let cand = format!("{cur}{w}");
            if cur.is_empty() || fits(&cand) {
                cur = cand;
            } else {
                lines.push(cur.trim_end().to_string());
                cur = w.trim_start().to_string();
            }
            if !fits(&cur) {
                cur = break_long(&cur, &mut lines);
            }
        }
        lines.push(cur.trim_end().to_string());
    }
    lines
}

fn line_height(font: &FieldFont, size: f64) -> f64 {
    size * ((font.ascent - font.descent) / 1000.0).max(1.15)
}

/// Everything the text generator needs about one widget showing `text`.
pub struct TextReq<'a> {
    pub field: &'a Field,
    pub widget: &'a Widget,
    pub text: &'a str,
    pub font: &'a FieldFont,
    pub da: &'a Da,
    pub color_override: Option<[f64; 3]>,
}

fn font_use(font: &FieldFont) -> FontUse {
    FontUse {
        name: font.res_name.clone(),
        obj: font.obj.clone(),
        zapf: false,
    }
}

/// Appearance for a text field or the text part of a combo box.
pub fn text_ap(r: &TextReq<'_>) -> Result<Ap> {
    let f = Frame::new(r.widget);
    let sess = r.font.session();
    if matches!(sess, Session::Unusable) {
        return Err(FillError::Appearance("font program unusable".into()));
    }
    let field = r.field;
    let mut text = r.text.to_string();
    if field.password() {
        text = "*".repeat(text.chars().count());
    }
    if !field.multiline() {
        text = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
    }
    if let Some(m) = field.max_len.filter(|_| field.comb()) {
        text = text.chars().take(m as usize).collect();
    }
    let mut lossy = false;
    let mut frame = ContentBuilder::new();
    draw_frame(&mut frame, &f, r.widget, false);
    let (tx_w, tx_h) = ((f.w - 2.0 * f.pad).max(1.0), (f.h - 2.0 * f.pad).max(1.0));
    let mut body = ContentBuilder::new();

    if !text.is_empty() {
        if field.comb() {
            let n = field.max_len.unwrap_or(1).max(1) as usize;
            let chars: Vec<char> = text.chars().collect();
            let cell = (f.w - 2.0 * f.bw) / n as f64;
            let size = if r.da.size > 0.0 {
                r.da.size
            } else {
                let widest = chars
                    .iter()
                    .map(|c| sess.measure(&c.to_string()))
                    .fold(0.0f64, f64::max);
                pick_size(|s| single_line_height(r.font, s) <= tx_h && pts(widest, s) <= cell - 1.0)
            };
            let start = match field.q {
                1 => (n - chars.len().min(n)) / 2,
                2 => n - chars.len().min(n),
                _ => 0,
            };
            let base_y = baseline_single(r.font, size, &f);
            body.begin_text().font(&r.font.res_name, size);
            apply_da_color(&mut body, r.da, r.color_override);
            let mut pen_x = 0.0;
            for (i, ch) in chars.iter().take(n).enumerate() {
                let (run, l) = sess.run_lossy(&ch.to_string());
                lossy |= l;
                let x = f.bw + (start + i) as f64 * cell + (cell - pts(run.width, size)) / 2.0;
                body.text_move(x - pen_x, if i == 0 { base_y } else { 0.0 });
                pen_x = x;
                show(&mut body, &run);
            }
            body.end_text();
        } else if field.multiline() {
            let size = if r.da.size > 0.0 {
                r.da.size
            } else {
                pick_size(|s| {
                    let lines = wrap(&sess, &text, s, tx_w);
                    lines.len() as f64 * line_height(r.font, s) <= tx_h
                })
                .min(12.0)
            };
            let lines = wrap(&sess, &text, size, tx_w);
            let lh = line_height(r.font, size);
            let mut y = f.h - f.pad - r.font.ascent * size / 1000.0;
            body.begin_text().font(&r.font.res_name, size);
            apply_da_color(&mut body, r.da, r.color_override);
            let (mut px, mut py) = (0.0, 0.0);
            for line in &lines {
                if y < -lh {
                    break; // below the box
                }
                let (run, l) = sess.run_lossy(line);
                lossy |= l;
                let x = aligned_x(field.q, f.w, f.pad, pts(run.width, size));
                body.text_move(x - px, y - py);
                (px, py) = (x, y);
                show(&mut body, &run);
                y -= lh;
            }
            body.end_text();
        } else {
            let (full, l) = sess.run_lossy(&text);
            lossy |= l;
            let size = if r.da.size > 0.0 {
                r.da.size
            } else {
                pick_size(|s| single_line_height(r.font, s) <= tx_h && pts(full.width, s) <= tx_w)
            };
            let x = aligned_x(field.q, f.w, f.pad, pts(full.width, size));
            let y = baseline_single(r.font, size, &f);
            body.begin_text().font(&r.font.res_name, size);
            apply_da_color(&mut body, r.da, r.color_override);
            body.text_move(x, y);
            show(&mut body, &full);
            body.end_text();
        }
    }

    // Marked content: frame, then the text clipped to the area inside the border, then (comb)
    // the cell dividers in the border colour.
    let mut out = tx_prefix();
    out.extend_from_slice(&frame.finish());
    let mut clip = ContentBuilder::new();
    clip.save()
        .rect(
            f.bw,
            f.bw,
            (f.w - 2.0 * f.bw).max(0.0),
            (f.h - 2.0 * f.bw).max(0.0),
        )
        .clip()
        .end_path();
    out.extend_from_slice(&clip.finish());
    out.extend_from_slice(&body.finish());
    out.extend_from_slice(b"Q\n");
    if field.comb()
        && let Some(bc) = r.widget.border_color.as_ref().filter(|c| !c.is_empty())
        && r.widget.border_width > 0.0
    {
        let n = field.max_len.unwrap_or(1).max(1) as usize;
        let cell = (f.w - 2.0 * f.bw) / n as f64;
        let mut d = ContentBuilder::new();
        d.save();
        set_color(&mut d, bc, false);
        d.line_width(f.bw.max(0.5));
        for i in 1..n {
            let x = f.bw + i as f64 * cell;
            d.move_to(x, f.bw).line_to(x, f.h - f.bw);
        }
        d.stroke().restore();
        out.extend_from_slice(&d.finish());
    }
    out.extend_from_slice(b"Q\nEMC\n");

    Ok(Ap {
        content: out,
        bbox: f.bbox,
        matrix: f.matrix,
        font: Some(font_use(r.font)),
        lossy,
    })
}

/// `/Tx BMC\nq\n` (the generator wraps the rest in `Q EMC`).
fn tx_prefix() -> Vec<u8> {
    b"/Tx BMC\nq\n".to_vec()
}

fn aligned_x(q: i64, w: f64, pad: f64, text_w: f64) -> f64 {
    let x = match q {
        1 => (w - text_w) / 2.0,
        2 => w - pad - text_w,
        _ => pad,
    };
    // Overflowing text starts at the left padding rather than off the box.
    x.max(pad)
}

fn single_line_height(font: &FieldFont, size: f64) -> f64 {
    (font.ascent - font.descent) / 1000.0 * size
}

/// Baseline that centres the font's ascent-to-descent box vertically.
fn baseline_single(font: &FieldFont, size: f64, f: &Frame) -> f64 {
    let (asc, desc) = (font.ascent * size / 1000.0, font.descent * size / 1000.0);
    (f.h - (asc - desc)) / 2.0 - desc
}

fn pick_size(fits: impl Fn(f64) -> bool) -> f64 {
    let mut best = 4.0;
    for &s in &AUTO_SIZES {
        if fits(s) {
            best = s;
        } else {
            break;
        }
    }
    best
}

/// Appearance of a list box: the visible options from `/TI`, selected ones highlighted.
pub fn list_ap(
    field: &Field,
    widget: &Widget,
    selected: &[usize],
    font: &FieldFont,
    da: &Da,
) -> Result<Ap> {
    let f = Frame::new(widget);
    let sess = font.session();
    if matches!(sess, Session::Unusable) {
        return Err(FillError::Appearance("font program unusable".into()));
    }
    let size = if da.size > 0.0 { da.size } else { 12.0 };
    let lh = line_height(font, size);
    let mut lossy = false;
    let mut out = tx_prefix();
    let mut b = ContentBuilder::new();
    draw_frame(&mut b, &f, widget, false);
    b.save()
        .rect(
            f.bw,
            f.bw,
            (f.w - 2.0 * f.bw).max(0.0),
            (f.h - 2.0 * f.bw).max(0.0),
        )
        .clip()
        .end_path();
    let top = f.h - f.bw;
    let first = field.top_index.unwrap_or(0) as usize;
    let rows = ((f.h - 2.0 * f.bw) / lh).ceil().max(1.0) as usize;
    // Highlights first so text draws over them.
    for row in 0..rows {
        let idx = first + row;
        if idx >= field.options.len() {
            break;
        }
        if selected.contains(&idx) {
            b.save()
                .fill_rgb(0.600_006, 0.756_866, 0.854_904)
                .rect(f.bw, top - (row as f64 + 1.0) * lh, f.w - 2.0 * f.bw, lh)
                .fill()
                .restore();
        }
    }
    b.begin_text().font(&font.res_name, size);
    apply_da_color(&mut b, da, None);
    let (mut px, mut py) = (0.0, 0.0);
    for row in 0..rows {
        let idx = first + row;
        let Some((_, display)) = field.options.get(idx) else {
            break;
        };
        let (run, l) = sess.run_lossy(display);
        lossy |= l;
        let base = top
            - (row as f64) * lh
            - (lh - single_line_height(font, size)) / 2.0
            - font.ascent * size / 1000.0;
        let x = f.pad;
        b.text_move(x - px, base - py);
        (px, py) = (x, base);
        show(&mut b, &run);
    }
    b.end_text();
    out.extend_from_slice(&b.finish());
    out.extend_from_slice(b"Q\nEMC\n");
    Ok(Ap {
        content: out,
        bbox: f.bbox,
        matrix: f.matrix,
        font: Some(font_use(font)),
        lossy,
    })
}

/// The mark ZapfDingbats code for a `/MK /CA` caption, with the kind's default.
pub fn mark_code(caption: Option<&str>, radio: bool) -> u8 {
    match caption.and_then(|c| c.bytes().next()) {
        Some(c @ (b'4' | b'8' | b'l' | b'n' | b'u' | b'H')) => c,
        _ if radio => b'l',
        _ => b'4',
    }
}

/// Checkbox or radio appearance: `on` draws the mark, otherwise only background and border.
pub fn button_ap(field: &Field, widget: &Widget, on: bool, da: &Da) -> Ap {
    let f = Frame::new(widget);
    let radio = field.kind == Kind::Radio;
    let code = mark_code(widget.caption.as_deref(), radio);
    let round = radio && code == b'l';
    let mut b = ContentBuilder::new();
    b.save();
    draw_frame(&mut b, &f, widget, round);
    b.restore();
    if on {
        let bb = ZAPF_BBOX[usize::from(code)];
        let (gw, gh) = (
            f64::from(bb[2] - bb[0]).max(1.0) / 1000.0,
            f64::from(bb[3] - bb[1]).max(1.0) / 1000.0,
        );
        let avail = (f.w.min(f.h) - 2.0 * f.bw - 1.0).max(1.0);
        let size = if da.size > 0.0 {
            da.size
        } else {
            (avail / gw).min(avail / gh)
        };
        let x = (f.w - gw * size) / 2.0 - f64::from(bb[0]) * size / 1000.0;
        let y = (f.h - gh * size) / 2.0 - f64::from(bb[1]) * size / 1000.0;
        b.save().begin_text().font("ZaDb", size);
        apply_da_color(&mut b, da, None);
        b.text_move(x, y).show_text([code]).end_text().restore();
    }
    let mut content = b"q\n".to_vec();
    content.extend_from_slice(&b.finish());
    content.extend_from_slice(b"Q\n");
    Ap {
        content,
        bbox: f.bbox,
        matrix: f.matrix,
        font: on.then(|| FontUse {
            name: "ZaDb".into(),
            obj: None,
            zapf: true,
        }),
        lossy: false,
    }
}

/// Free-standing text for flat-form annotations: no frame, wrapped to `width`, top aligned.
pub struct FreeTextLayout {
    pub ap: Ap,
    pub width: f64,
    pub height: f64,
}

pub fn free_text_ap(
    text: &str,
    font: &FieldFont,
    size: f64,
    color: &[f64],
    width: Option<f64>,
) -> Result<FreeTextLayout> {
    let sess = font.session();
    if matches!(sess, Session::Unusable) {
        return Err(FillError::Appearance("font program unusable".into()));
    }
    let pad = 1.0;
    let lines = match width {
        Some(w) => wrap(&sess, text, size, (w - 2.0 * pad).max(1.0)),
        None => text
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .split('\n')
            .map(str::to_string)
            .collect(),
    };
    let mut runs = Vec::new();
    let mut max_w = 0.0f64;
    let mut lossy = false;
    for l in &lines {
        let (r, lo) = sess.run_lossy(l);
        lossy |= lo;
        max_w = max_w.max(pts(r.width, size));
        runs.push(r);
    }
    let lh = line_height(font, size);
    let w = width.unwrap_or(max_w + 2.0 * pad);
    let h = lines.len() as f64 * lh + 2.0 * pad;
    let mut b = ContentBuilder::new();
    b.begin_text().font(&font.res_name, size);
    set_color(&mut b, if color.is_empty() { &[0.0] } else { color }, true);
    let mut y = h - pad - font.ascent * size / 1000.0;
    let (mut px, mut py) = (0.0, 0.0);
    for r in &runs {
        let x = pad;
        b.text_move(x - px, y - py);
        (px, py) = (x, y);
        show(&mut b, r);
        y -= lh;
    }
    b.end_text();
    Ok(FreeTextLayout {
        ap: Ap {
            content: b.finish(),
            bbox: [0.0, 0.0, w, h],
            matrix: None,
            font: Some(font_use(font)),
            lossy,
        },
        width: w,
        height: h,
    })
}

/// A centred ZapfDingbats mark (check, cross, bullet) of roughly `size` points square.
pub fn mark_ap(code: u8, size: f64, color: &[f64]) -> Ap {
    let bb = ZAPF_BBOX[usize::from(code)];
    let (gw, gh) = (
        f64::from(bb[2] - bb[0]).max(1.0) / 1000.0,
        f64::from(bb[3] - bb[1]).max(1.0) / 1000.0,
    );
    let fs = (size / gw).min(size / gh);
    let x = (size - gw * fs) / 2.0 - f64::from(bb[0]) * fs / 1000.0;
    let y = (size - gh * fs) / 2.0 - f64::from(bb[1]) * fs / 1000.0;
    let mut b = ContentBuilder::new();
    b.begin_text().font("ZaDb", fs);
    set_color(&mut b, if color.is_empty() { &[0.0] } else { color }, true);
    b.text_move(x, y).show_text([code]).end_text();
    Ap {
        content: b.finish(),
        bbox: [0.0, 0.0, size, size],
        matrix: None,
        font: Some(FontUse {
            name: "ZaDb".into(),
            obj: None,
            zapf: true,
        }),
        lossy: false,
    }
}
