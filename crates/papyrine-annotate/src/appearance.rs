//! Appearance streams per type (PDF 32000-1 section 12.5.5), as pure data.

use papyrine_content::{ContentBuilder, Object as Cobj, TextItem};
use papyrine_core::Matrix;
use papyrine_ops::Result;

use crate::geometry::{
    Geometry, LineEnding, MarkupKind, Pt, Quad, TextStyle, bounds_of, normalize_rect,
};
use crate::props::Color;
use crate::spec::Spec;
use crate::text::embed::{PreparedFont, prepare};
use crate::text::{FontReport, Layout, layout};

/// Size of a sticky-note icon in points.
pub const NOTE_SIZE: f64 = 20.0;
const KAPPA: f64 = 0.552_284_749_830_793_4;

/// Everything needed to write the annotation's `/Rect` and `/AP /N` stream.
pub struct Appearance {
    /// `/Rect`.
    pub rect: [f64; 4],
    /// Form `/BBox` (equal to `rect`: content is in page coordinates).
    pub bbox: [f64; 4],
    pub content: Vec<u8>,
    /// ExtGState opacity (`/CA` and `/ca`); `None` when fully opaque and not multiplied.
    pub opacity: Option<f64>,
    /// Blend mode Multiply (highlights).
    pub multiply: bool,
    pub fonts: Vec<PreparedFont>,
    /// `/RD` for shapes and text boxes.
    pub rd: Option<[f64; 4]>,
    pub report: FontReport,
}

fn sub(a: Pt, b: Pt) -> Pt {
    [a[0] - b[0], a[1] - b[1]]
}
fn add(a: Pt, b: Pt) -> Pt {
    [a[0] + b[0], a[1] + b[1]]
}
fn mul(a: Pt, k: f64) -> Pt {
    [a[0] * k, a[1] * k]
}
fn len(a: Pt) -> f64 {
    a[0].hypot(a[1])
}
fn unit(a: Pt) -> Pt {
    let l = len(a);
    if l < 1e-9 {
        [1.0, 0.0]
    } else {
        [a[0] / l, a[1] / l]
    }
}
fn dot(a: Pt, b: Pt) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn mid(a: Pt, b: Pt) -> Pt {
    mul(add(a, b), 0.5)
}
fn inflate(r: [f64; 4], d: f64) -> [f64; 4] {
    [r[0] - d, r[1] - d, r[2] + d, r[3] + d]
}

pub fn build(spec: &Spec) -> Result<Appearance> {
    spec.validate()?;
    let a = match &spec.geometry {
        Geometry::TextMarkup { kind, quads } => markup(spec, *kind, quads),
        Geometry::Note { pos, icon } => note(spec, *pos, icon),
        Geometry::TextBox { rect, style } => text_box(spec, *rect, style)?,
        Geometry::Ink { strokes } => ink(spec, strokes),
        Geometry::Square { rect } => shape(spec, *rect, false),
        Geometry::Circle { rect } => shape(spec, *rect, true),
        Geometry::Line {
            from,
            to,
            start,
            end,
        } => line(spec, *from, *to, *start, *end),
    };
    Ok(a)
}

fn new_app(rect: [f64; 4], b: ContentBuilder, spec: &Spec, multiply: bool) -> Appearance {
    debug_assert!(b.is_balanced());
    let o = spec.opacity();
    Appearance {
        rect,
        bbox: rect,
        content: b.finish(),
        opacity: ((o - 1.0).abs() > 1e-9).then_some(o),
        multiply,
        fonts: Vec::new(),
        rd: None,
        report: FontReport::default(),
    }
}

fn start(spec: &Spec, multiply: bool) -> ContentBuilder {
    let mut b = ContentBuilder::new();
    b.save();
    if multiply || (spec.opacity() - 1.0).abs() > 1e-9 {
        b.ext_gstate("GS0");
    }
    b
}

fn apply_dash(b: &mut ContentBuilder, spec: &Spec) {
    if !spec.props.dash.is_empty() {
        b.dash(&spec.props.dash, 0.0);
    }
}

// --- text markup ---------------------------------------------------------------------------

fn markup(spec: &Spec, kind: MarkupKind, quads: &[Quad]) -> Appearance {
    let w = spec.width().max(0.25);
    let color = spec.color();
    let bounds = bounds_of(quads.iter().flat_map(|q| q.0.iter().copied())).unwrap_or([0.0; 4]);
    let rect = match kind {
        MarkupKind::Highlight => bounds,
        _ => inflate(bounds, w.max(1.0) + 1.0),
    };
    let mut b = start(spec, kind == MarkupKind::Highlight);
    match kind {
        MarkupKind::Highlight => {
            color.set_fill(&mut b);
            for q in quads {
                let p = q.0;
                b.move_to(p[0][0], p[0][1])
                    .line_to(p[1][0], p[1][1])
                    .line_to(p[3][0], p[3][1])
                    .line_to(p[2][0], p[2][1])
                    .close()
                    .fill();
            }
        }
        _ => {
            color.set_stroke(&mut b);
            b.line_width(w).line_cap(0).line_join(0);
            for q in quads {
                stroke_markup_line(&mut b, kind, &q.0, w);
            }
        }
    }
    b.restore();
    new_app(rect, b, spec, kind == MarkupKind::Highlight)
}

/// Baseline direction and the unit normal pointing from the baseline towards the text.
fn quad_frame(p: &[Pt; 4]) -> (Pt, Pt) {
    let mut u = sub(p[3], p[2]);
    if len(u) < 1e-9 {
        u = sub(p[1], p[0]);
    }
    let u = unit(u);
    let mut n = [-u[1], u[0]];
    if dot(n, sub(p[0], p[2])) < 0.0 {
        n = mul(n, -1.0);
    }
    (u, n)
}

fn stroke_markup_line(b: &mut ContentBuilder, kind: MarkupKind, p: &[Pt; 4], w: f64) {
    let (u, n) = quad_frame(p);
    match kind {
        MarkupKind::Underline => {
            let off = mul(n, w);
            let (a, c) = (add(p[2], off), add(p[3], off));
            b.move_to(a[0], a[1]).line_to(c[0], c[1]).stroke();
        }
        MarkupKind::StrikeOut => {
            let (a, c) = (mid(p[0], p[2]), mid(p[1], p[3]));
            b.move_to(a[0], a[1]).line_to(c[0], c[1]).stroke();
        }
        MarkupKind::Squiggly => {
            let base = add(p[2], mul(n, w));
            let total = len(sub(p[3], p[2]));
            let step = (2.0 * w).max(2.0);
            let amp = w.max(1.0);
            let count = (total / step).ceil().max(1.0) as usize;
            b.move_to(base[0], base[1]);
            for i in 1..=count {
                let along = (i as f64 * step).min(total);
                let up = if i % 2 == 1 { amp } else { 0.0 };
                let pt = add(add(base, mul(u, along)), mul(n, up));
                b.line_to(pt[0], pt[1]);
            }
            b.stroke();
        }
        MarkupKind::Highlight => {}
    }
}

// --- sticky note ---------------------------------------------------------------------------

fn note(spec: &Spec, pos: Pt, icon: &str) -> Appearance {
    let rect = [pos[0], pos[1] - NOTE_SIZE, pos[0] + NOTE_SIZE, pos[1]];
    let mut b = start(spec, false);
    b.concat_matrix(&Matrix::translate(rect[0], rect[1]));
    b.line_width(0.8).line_join(1).line_cap(1);
    spec.color().set_fill(&mut b);
    b.stroke_gray(0.0);
    match icon {
        "Comment" => {
            b.move_to(2.0, 6.0)
                .line_to(5.5, 6.0)
                .line_to(4.5, 2.0)
                .line_to(9.5, 6.0)
                .line_to(18.0, 6.0)
                .line_to(18.0, 18.0)
                .line_to(2.0, 18.0)
                .close()
                .fill_stroke();
            for y in [9.5, 12.5, 15.0] {
                b.move_to(5.0, y).line_to(15.0, y).stroke();
            }
        }
        "Help" => {
            circle_path(&mut b, 10.0, 10.0, 8.0, 8.0);
            b.fill_stroke();
            b.line_width(1.6).stroke_gray(0.0);
            b.move_to(7.2, 12.5)
                .curve_to(7.2, 15.6, 12.8, 15.6, 12.8, 12.5)
                .curve_to(12.8, 10.8, 10.0, 10.6, 10.0, 8.4)
                .stroke();
            b.fill_gray(0.0);
            circle_path(&mut b, 10.0, 5.8, 1.0, 1.0);
            b.fill();
        }
        "Key" => {
            circle_path(&mut b, 6.0, 13.0, 4.0, 4.0);
            b.fill_stroke();
            b.line_width(1.6)
                .move_to(8.8, 10.2)
                .line_to(17.5, 1.8)
                .stroke();
            b.move_to(14.5, 4.8).line_to(16.5, 6.6).stroke();
            b.move_to(12.0, 7.4).line_to(13.8, 9.2).stroke();
        }
        "Insert" => {
            b.move_to(10.0, 17.0)
                .line_to(18.0, 3.0)
                .line_to(2.0, 3.0)
                .close()
                .fill_stroke();
        }
        _ => {
            // Note: a page with a folded corner and three lines of text.
            b.move_to(2.0, 2.0)
                .line_to(18.0, 2.0)
                .line_to(18.0, 13.0)
                .line_to(13.0, 18.0)
                .line_to(2.0, 18.0)
                .close()
                .fill_stroke();
            b.move_to(13.0, 18.0)
                .line_to(13.0, 13.0)
                .line_to(18.0, 13.0)
                .stroke();
            for y in [5.0, 8.0, 11.0] {
                b.move_to(5.0, y).line_to(13.0, y).stroke();
            }
        }
    }
    b.restore();
    new_app(rect, b, spec, false)
}

fn circle_path(b: &mut ContentBuilder, cx: f64, cy: f64, rx: f64, ry: f64) {
    let (kx, ky) = (rx * KAPPA, ry * KAPPA);
    b.move_to(cx + rx, cy)
        .curve_to(cx + rx, cy + ky, cx + kx, cy + ry, cx, cy + ry)
        .curve_to(cx - kx, cy + ry, cx - rx, cy + ky, cx - rx, cy)
        .curve_to(cx - rx, cy - ky, cx - kx, cy - ry, cx, cy - ry)
        .curve_to(cx + kx, cy - ry, cx + rx, cy - ky, cx + rx, cy)
        .close();
}

// --- ink -----------------------------------------------------------------------------------

fn ink(spec: &Spec, strokes: &[Vec<Pt>]) -> Appearance {
    let w = spec.width().max(0.25);
    let bounds = bounds_of(strokes.iter().flat_map(|s| s.iter().copied())).unwrap_or([0.0; 4]);
    let rect = inflate(bounds, w / 2.0 + 1.0);
    let mut b = start(spec, false);
    spec.color().set_stroke(&mut b);
    b.line_width(w).line_cap(1).line_join(1);
    apply_dash(&mut b, spec);
    for s in strokes {
        smooth_path(&mut b, s);
        b.stroke();
    }
    b.restore();
    new_app(rect, b, spec, false)
}

/// Polyline through the points, smoothed with midpoint quadratic curves.
fn smooth_path(b: &mut ContentBuilder, s: &[Pt]) {
    match s {
        [] => {}
        [p] => {
            // A dot: a zero-length segment with round caps.
            b.move_to(p[0], p[1]).line_to(p[0] + 0.01, p[1]);
        }
        [p, q] => {
            b.move_to(p[0], p[1]).line_to(q[0], q[1]);
        }
        _ => {
            b.move_to(s[0][0], s[0][1]);
            let mut cur = s[0];
            for i in 1..s.len() - 1 {
                let ctrl = s[i];
                let end = mid(s[i], s[i + 1]);
                let c1 = add(cur, mul(sub(ctrl, cur), 2.0 / 3.0));
                let c2 = add(end, mul(sub(ctrl, end), 2.0 / 3.0));
                b.curve_to(c1[0], c1[1], c2[0], c2[1], end[0], end[1]);
                cur = end;
            }
            let last = s[s.len() - 1];
            b.line_to(last[0], last[1]);
        }
    }
}

// --- square / circle -----------------------------------------------------------------------

fn shape(spec: &Spec, rect: [f64; 4], oval: bool) -> Appearance {
    let rect = normalize_rect(rect);
    let w = spec.width();
    let half = w / 2.0;
    let mut b = start(spec, false);
    let fill = spec.props.fill.clone();
    if let Some(f) = &fill {
        f.set_fill(&mut b);
    }
    if w > 0.0 {
        spec.color().set_stroke(&mut b);
        b.line_width(w).line_join(0);
        apply_dash(&mut b, spec);
    }
    let (x0, y0, x1, y1) = (
        rect[0] + half,
        rect[1] + half,
        rect[2] - half,
        rect[3] - half,
    );
    if oval {
        circle_path(
            &mut b,
            (x0 + x1) / 2.0,
            (y0 + y1) / 2.0,
            (x1 - x0) / 2.0,
            (y1 - y0) / 2.0,
        );
    } else {
        b.rect(x0, y0, x1 - x0, y1 - y0);
    }
    match (fill.is_some(), w > 0.0) {
        (true, true) => b.fill_stroke(),
        (true, false) => b.fill(),
        (false, true) => b.stroke(),
        (false, false) => b.end_path(),
    };
    b.restore();
    let mut a = new_app(rect, b, spec, false);
    a.rd = Some([half; 4]);
    a
}

// --- line / arrow --------------------------------------------------------------------------

fn ending_size(w: f64) -> f64 {
    (w * 4.5).max(6.0)
}

/// Draw a line ending whose tip/anchor is `at` and whose direction (pointing away from the
/// line body, outwards) is `dir`. Returns where the body line should stop, plus points for the
/// bounding box.
fn draw_ending(
    b: &mut ContentBuilder,
    kind: LineEnding,
    at: Pt,
    dir: Pt,
    w: f64,
    fill: bool,
    pts: &mut Vec<Pt>,
) -> Pt {
    let l = ending_size(w);
    let n = [-dir[1], dir[0]];
    let paint = |b: &mut ContentBuilder| {
        if kind.is_closed() && fill {
            b.close().fill_stroke();
        } else if kind.is_closed() {
            b.close_stroke();
        } else {
            b.stroke();
        }
    };
    match kind {
        LineEnding::None => at,
        LineEnding::OpenArrow | LineEnding::ClosedArrow => {
            let base = sub(at, mul(dir, l));
            let (a, c) = (add(base, mul(n, l * 0.4)), sub(base, mul(n, l * 0.4)));
            b.move_to(a[0], a[1])
                .line_to(at[0], at[1])
                .line_to(c[0], c[1]);
            paint(b);
            pts.extend([a, c, at]);
            if kind == LineEnding::ClosedArrow {
                base
            } else {
                at
            }
        }
        LineEnding::ROpenArrow | LineEnding::RClosedArrow => {
            let tip = sub(at, mul(dir, l));
            let (a, c) = (add(at, mul(n, l * 0.4)), sub(at, mul(n, l * 0.4)));
            b.move_to(a[0], a[1])
                .line_to(tip[0], tip[1])
                .line_to(c[0], c[1]);
            paint(b);
            pts.extend([a, c, tip]);
            at
        }
        LineEnding::Butt => {
            let (a, c) = (add(at, mul(n, l * 0.5)), sub(at, mul(n, l * 0.5)));
            b.move_to(a[0], a[1]).line_to(c[0], c[1]).stroke();
            pts.extend([a, c]);
            at
        }
        LineEnding::Slash => {
            let d = add(mul(n, 0.5), mul(dir, 0.866));
            let (a, c) = (add(at, mul(d, l * 0.5)), sub(at, mul(d, l * 0.5)));
            b.move_to(a[0], a[1]).line_to(c[0], c[1]).stroke();
            pts.extend([a, c]);
            at
        }
        LineEnding::Square => {
            let h = l / 2.0;
            let c = [
                add(add(at, mul(n, h)), mul(dir, h)),
                add(sub(at, mul(n, h)), mul(dir, h)),
                sub(sub(at, mul(n, h)), mul(dir, h)),
                sub(add(at, mul(n, h)), mul(dir, h)),
            ];
            b.move_to(c[0][0], c[0][1]);
            for p in &c[1..] {
                b.line_to(p[0], p[1]);
            }
            paint(b);
            pts.extend(c);
            sub(at, mul(dir, h))
        }
        LineEnding::Diamond => {
            let h = l / 2.0;
            let c = [
                add(at, mul(dir, h)),
                add(at, mul(n, h)),
                sub(at, mul(dir, h)),
                sub(at, mul(n, h)),
            ];
            b.move_to(c[0][0], c[0][1]);
            for p in &c[1..] {
                b.line_to(p[0], p[1]);
            }
            paint(b);
            pts.extend(c);
            sub(at, mul(dir, h))
        }
        LineEnding::Circle => {
            let h = l / 2.0;
            circle_path(b, at[0], at[1], h, h);
            if fill {
                b.fill_stroke();
            } else {
                b.stroke();
            }
            pts.extend([add(at, [h, h]), add(at, [-h, -h])]);
            sub(at, mul(dir, h))
        }
    }
}

fn line(spec: &Spec, from: Pt, to: Pt, start_e: LineEnding, end_e: LineEnding) -> Appearance {
    let w = spec.width().max(0.25);
    let mut b = start(spec, false);
    spec.color().set_stroke(&mut b);
    let fill = spec.props.fill.clone();
    match &fill {
        Some(f) => f.set_fill(&mut b),
        None => {
            b.fill_gray(1.0);
        }
    }
    b.line_width(w).line_cap(0).line_join(0);
    let d = unit(sub(to, from));
    let mut pts = vec![from, to];
    // Endings first (solid), then the body so dashes do not affect them.
    let has_fill = fill.is_some();
    let body_from = draw_ending(&mut b, start_e, from, mul(d, -1.0), w, has_fill, &mut pts);
    let body_to = draw_ending(&mut b, end_e, to, d, w, has_fill, &mut pts);
    apply_dash(&mut b, spec);
    b.move_to(body_from[0], body_from[1])
        .line_to(body_to[0], body_to[1])
        .stroke();
    b.restore();
    let bounds = bounds_of(pts.into_iter()).unwrap_or([0.0; 4]);
    new_app(inflate(bounds, w / 2.0 + 1.0), b, spec, false)
}

// --- text box ------------------------------------------------------------------------------

/// Height needed (points) to show `text` in a box of width `box_width` with `style`.
pub fn measure_text_box(text: &str, style: &TextStyle, box_width: f64, border: f64) -> f64 {
    let pad = 2.0 + border;
    let lay = layout(text, style, (box_width - 2.0 * pad).max(1.0));
    lay.text_height().max(style.size) + 2.0 * pad
}

fn text_box(spec: &Spec, rect: [f64; 4], style: &TextStyle) -> Result<Appearance> {
    let rect = normalize_rect(rect);
    let bw = spec.width();
    let pad = 2.0 + bw;
    let (w, h) = (rect[2] - rect[0], rect[3] - rect[1]);
    let inner_w = (w - 2.0 * pad).max(1.0);
    let text = spec.props.contents.clone().unwrap_or_default();
    let lay = layout(&text, style, inner_w);
    let fonts = prepare(&lay)?;

    let mut b = start(spec, false);
    if let Some(f) = &spec.props.fill {
        f.set_fill(&mut b);
        b.rect(rect[0], rect[1], w, h).fill();
    }
    if bw > 0.0 {
        spec.color().set_stroke(&mut b);
        b.line_width(bw);
        apply_dash(&mut b, spec);
        b.rect(rect[0] + bw / 2.0, rect[1] + bw / 2.0, w - bw, h - bw)
            .stroke();
    }
    if !text.is_empty() {
        b.save();
        b.rect(
            rect[0] + bw,
            rect[1] + bw,
            (w - 2.0 * bw).max(0.0),
            (h - 2.0 * bw).max(0.0),
        )
        .clip()
        .end_path();
        style.color.set_fill(&mut b);
        emit_text(&mut b, &lay, &fonts, style, rect, pad, inner_w);
        b.restore();
    }
    b.restore();
    let mut a = new_app(rect, b, spec, false);
    a.report = lay.report.clone();
    a.fonts = fonts;
    Ok(a)
}

fn emit_text(
    b: &mut ContentBuilder,
    lay: &Layout,
    fonts: &[PreparedFont],
    style: &TextStyle,
    rect: [f64; 4],
    pad: f64,
    inner_w: f64,
) {
    let font_of = |face: usize| fonts.iter().find(|f| f.face == face);
    for (li, line) in lay.lines.iter().enumerate() {
        if line.glyphs.is_empty() {
            continue;
        }
        let baseline = rect[3] - pad - lay.ascent - li as f64 * lay.line_height;
        let x0 = rect[0]
            + pad
            + match style.align {
                crate::geometry::Align::Left => 0.0,
                crate::geometry::Align::Center => (inner_w - line.width) / 2.0,
                crate::geometry::Align::Right => inner_w - line.width,
            };
        if line.has_rtl {
            let mut utf16 = vec![0xFE, 0xFF];
            for u in line.text.encode_utf16() {
                utf16.extend_from_slice(&u.to_be_bytes());
            }
            b.op(
                "BDC",
                vec![
                    Cobj::name("Span"),
                    Cobj::Dict(vec![(b"ActualText".to_vec(), Cobj::Str(utf16))]),
                ],
            );
        }
        b.begin_text();
        let mut i = 0;
        while i < line.glyphs.len() {
            let g0 = &line.glyphs[i];
            let Some(font) = font_of(g0.face) else {
                i += 1;
                continue;
            };
            let positioned = g0.dx != 0.0 || g0.dy != 0.0;
            let mut j = i + 1;
            if !positioned {
                while j < line.glyphs.len() {
                    let g = &line.glyphs[j];
                    if g.face != g0.face || g.dx != 0.0 || g.dy != 0.0 {
                        break;
                    }
                    j += 1;
                }
            }
            b.font(&font.resource, style.size);
            b.text_matrix(&Matrix::translate(x0 + g0.x + g0.dx, baseline + g0.dy));
            let mut items: Vec<TextItem> = Vec::new();
            let mut cur: Vec<u8> = Vec::new();
            for (k, g) in line.glyphs[i..j].iter().enumerate() {
                let cid = font.remap.get(&g.gid).copied().unwrap_or(0);
                cur.extend_from_slice(&cid.to_be_bytes());
                if i + k + 1 < j {
                    let w1000 = font.widths.get(&cid).copied().unwrap_or(1000) as f64;
                    let shaped = g.advance / style.size * 1000.0;
                    let adj = w1000 - shaped;
                    if adj.abs() >= 0.05 {
                        items.push(TextItem::Text(std::mem::take(&mut cur)));
                        items.push(TextItem::Adjust((adj * 100.0).round() / 100.0));
                    }
                }
            }
            items.push(TextItem::Text(cur));
            b.show_text_adjusted(&items);
            i = j;
        }
        b.end_text();
        if line.has_rtl {
            b.op("EMC", vec![]);
        }
    }
}

/// Convenience used by tests and the UI: the XHTML rich-text value for `/RC`.
pub fn rich_content(text: &str, style: &TextStyle) -> String {
    let (family, weight) = (
        match style.family {
            crate::geometry::FontFamily::Sans => "Helvetica,sans-serif",
            crate::geometry::FontFamily::Serif => "Times New Roman,serif",
            crate::geometry::FontFamily::Mono => "Courier New,monospace",
        },
        if style.bold { "bold" } else { "normal" },
    );
    let rgb = style.color.to_rgb();
    let hex = format!(
        "#{:02X}{:02X}{:02X}",
        (rgb[0] * 255.0).round() as u8,
        (rgb[1] * 255.0).round() as u8,
        (rgb[2] * 255.0).round() as u8
    );
    let align = match style.align {
        crate::geometry::Align::Left => "left",
        crate::geometry::Align::Center => "center",
        crate::geometry::Align::Right => "right",
    };
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let mut out = format!(
        "<?xml version=\"1.0\"?><body xmlns=\"http://www.w3.org/1999/xhtml\" \
         xmlns:xfa=\"http://www.xfa.org/schema/xfa-data/1.0/\" xfa:APIVersion=\"Acrobat:11.0.0\" \
         xfa:spec=\"2.0.2\" style=\"font-size:{}pt;text-align:{align};color:{hex};\
         font-weight:{weight};font-style:normal;font-family:{family};font-stretch:normal\">",
        style.size
    );
    for p in text.replace("\r\n", "\n").split('\n') {
        out.push_str(&format!(
            "<p dir=\"ltr\"><span style=\"font-family:{family}\">{}</span></p>",
            esc(p)
        ));
    }
    out.push_str("</body>");
    out
}

/// The `/DA` string for a style: standard font alias, size and colour operator.
pub fn default_appearance(style: &TextStyle) -> String {
    let font = match (style.family, style.bold) {
        (crate::geometry::FontFamily::Sans, false) => "Helv",
        (crate::geometry::FontFamily::Sans, true) => "HeBo",
        (crate::geometry::FontFamily::Serif, false) => "TiRo",
        (crate::geometry::FontFamily::Serif, true) => "TiBo",
        (crate::geometry::FontFamily::Mono, false) => "Cour",
        (crate::geometry::FontFamily::Mono, true) => "CoBo",
    };
    let col = |c: &Color| {
        let f = |v: &f64| {
            let s = format!("{:.4}", v);
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        };
        match c.0.len() {
            1 => format!("{} g", f(&c.0[0])),
            4 => format!(
                "{} {} {} {} k",
                f(&c.0[0]),
                f(&c.0[1]),
                f(&c.0[2]),
                f(&c.0[3])
            ),
            _ => {
                let v = c.to_rgb();
                format!("{} {} {} rg", f(&v[0]), f(&v[1]), f(&v[2]))
            }
        }
    };
    format!("/{font} {} Tf {}", style.size, col(&style.color))
}

/// The `/DS` default style string.
pub fn default_style_string(style: &TextStyle) -> String {
    let family = match style.family {
        crate::geometry::FontFamily::Sans => "Helvetica,sans-serif",
        crate::geometry::FontFamily::Serif => "Times New Roman,serif",
        crate::geometry::FontFamily::Mono => "Courier New,monospace",
    };
    let rgb = style.color.to_rgb();
    format!(
        "font: {family} {}pt{}; text-align:{}; color:#{:02X}{:02X}{:02X}",
        style.size,
        if style.bold { " bold" } else { "" },
        match style.align {
            crate::geometry::Align::Left => "left",
            crate::geometry::Align::Center => "center",
            crate::geometry::Align::Right => "right",
        },
        (rgb[0] * 255.0).round() as u8,
        (rgb[1] * 255.0).round() as u8,
        (rgb[2] * 255.0).round() as u8
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{Align, FontFamily};
    use crate::props::AnnotProps;

    fn text_of(spec: &Spec) -> String {
        String::from_utf8(build(spec).unwrap().content).unwrap()
    }

    #[test]
    fn highlight_fills_the_quad_in_z_order_with_multiply() {
        let spec = Spec::new(
            Geometry::highlight(vec![Quad::from_rect(10.0, 20.0, 110.0, 40.0)]),
            AnnotProps::default(),
        );
        let a = build(&spec).unwrap();
        assert!(a.multiply && a.opacity.is_none());
        assert_eq!(a.rect, [10.0, 20.0, 110.0, 40.0]);
        // p1 p2 p4 p3: top-left, top-right, bottom-right, bottom-left.
        assert_eq!(
            String::from_utf8(a.content).unwrap(),
            "q\n/GS0 gs\n1 1 0 rg\n10 40 m\n110 40 l\n110 20 l\n10 20 l\nh\nf\nQ\n"
        );
    }

    #[test]
    fn shapes_inset_the_path_by_half_the_border() {
        let spec = Spec::new(
            Geometry::Square {
                rect: [0.0, 0.0, 100.0, 50.0],
            },
            AnnotProps::default().with_width(4.0),
        );
        let a = build(&spec).unwrap();
        assert_eq!(a.rd, Some([2.0; 4]));
        assert!(
            String::from_utf8(a.content)
                .unwrap()
                .contains("2 2 96 46 re")
        );
    }

    #[test]
    fn opacity_sets_the_graphics_state() {
        let spec = Spec::new(
            Geometry::Circle {
                rect: [0.0, 0.0, 10.0, 10.0],
            },
            AnnotProps::default().with_opacity(0.4),
        );
        let a = build(&spec).unwrap();
        assert_eq!(a.opacity, Some(0.4));
        assert!(text_of(&spec).contains("/GS0 gs"));
        assert!(!a.multiply);
    }

    #[test]
    fn markup_rect_covers_the_stroke() {
        let q = Quad::from_rect(10.0, 10.0, 60.0, 30.0);
        let spec = Spec::new(
            Geometry::underline(vec![q]),
            AnnotProps::default().with_width(3.0),
        );
        let a = build(&spec).unwrap();
        assert!(a.rect[0] < 10.0 && a.rect[1] < 10.0 && a.rect[2] > 60.0 && a.rect[3] > 30.0);
    }

    #[test]
    fn da_and_ds_and_rc_strings() {
        let st = TextStyle {
            family: FontFamily::Mono,
            bold: true,
            size: 9.5,
            color: Color::rgb(1.0, 0.0, 0.5),
            align: Align::Right,
        };
        assert_eq!(default_appearance(&st), "/CoBo 9.5 Tf 1 0 0.5 rg");
        assert_eq!(
            default_style_string(&st),
            "font: Courier New,monospace 9.5pt bold; text-align:right; color:#FF0080"
        );
        let rc = rich_content("a <b> & c\nline2", &st);
        assert!(rc.contains("a &lt;b&gt; &amp; c"));
        assert_eq!(rc.matches("<p ").count(), 2);
        assert_eq!(
            crate::objects::parse_da(&default_appearance(&st)),
            st.clone().with_align_default()
        );
    }

    trait AlignDefault {
        fn with_align_default(self) -> Self;
    }
    impl AlignDefault for TextStyle {
        // `/DA` carries no alignment (that is `/Q`).
        fn with_align_default(mut self) -> Self {
            self.align = Align::Left;
            self
        }
    }

    #[test]
    fn line_endings_extend_the_rect() {
        use LineEnding::*;
        let plain = build(&Spec::new(
            Geometry::line([0.0, 0.0], [100.0, 0.0]),
            AnnotProps::default(),
        ))
        .unwrap();
        let arrow = build(&Spec::new(
            Geometry::Line {
                from: [0.0, 0.0],
                to: [100.0, 0.0],
                start: ClosedArrow,
                end: ClosedArrow,
            },
            AnnotProps::default(),
        ))
        .unwrap();
        assert!(
            arrow.rect[3] > plain.rect[3] + 2.0,
            "{:?} vs {:?}",
            arrow.rect,
            plain.rect
        );
    }

    #[test]
    fn balanced_for_every_type() {
        let p = || AnnotProps::default().with_fill(Color::gray(0.9));
        for g in [
            Geometry::highlight(vec![Quad::from_rect(0.0, 0.0, 9.0, 9.0)]),
            Geometry::squiggly(vec![Quad::from_rect(0.0, 0.0, 99.0, 9.0)]),
            Geometry::note(10.0, 30.0),
            Geometry::Ink {
                strokes: vec![
                    vec![[0.0, 0.0]],
                    vec![[0.0, 0.0], [5.0, 5.0]],
                    vec![[0.0, 0.0], [5.0, 5.0], [9.0, 0.0], [12.0, 7.0]],
                ],
            },
            Geometry::Square {
                rect: [0.0, 0.0, 9.0, 9.0],
            },
            Geometry::arrow([0.0, 0.0], [20.0, 20.0]),
        ] {
            let a = build(&Spec::new(g, p())).unwrap();
            let parsed = papyrine_content::parse(&a.content);
            assert!(parsed.errors.is_empty());
            let (mut q, mut bt) = (0i32, 0i32);
            for op in &parsed.ops {
                match op.operator.as_slice() {
                    b"q" => q += 1,
                    b"Q" => q -= 1,
                    b"BT" => bt += 1,
                    b"ET" => bt -= 1,
                    _ => {}
                }
                assert!(q >= 0 && bt >= 0);
            }
            assert_eq!((q, bt), (0, 0));
        }
    }
}
