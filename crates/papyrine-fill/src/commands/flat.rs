//! Flat forms: PDFs with no fields. Click-to-type text and the check, cross and bullet marks are
//! FreeText annotations with no border (ISO 32000-2 section 12.5.6.6), typewriter intent, so
//! every viewer shows them and they stay editable as annotations.

use papyrine_cos::{Document, Object, ObjectKind};
use papyrine_ops::{ChangeSet, Command, EditContext, Error, LocalizedText, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::appearance::{self, Ap};
use crate::cosx;
use crate::error::FillError;
use crate::font::{self, FieldFont};
use crate::install;

fn rotation_of(page: &Object) -> u16 {
    cosx::inherited(page, "Rotate")
        .as_ref()
        .and_then(cosx::int)
        .map_or(0, |r| (r.rem_euclid(360) / 90 * 90) as u16)
}

/// Annotation rectangle of a displayed `w` x `h` box whose displayed top-left corner is the user
/// space point (`ax`, `ay`), on a page rotated by `rot`; plus the AP matrix that keeps content
/// upright on screen.
fn place(rot: u16, ax: f64, ay: f64, w: f64, h: f64) -> ([f64; 4], Option<[f64; 6]>, [f64; 4]) {
    let (rect, bbox_dims) = match rot {
        90 => ([ax, ay, ax + h, ay + w], (w, h)),
        180 => ([ax - w, ay, ax, ay + h], (w, h)),
        270 => ([ax - h, ay - w, ax, ay], (w, h)),
        _ => ([ax, ay - h, ax + w, ay], (w, h)),
    };
    let (rw, rh) = (rect[2] - rect[0], rect[3] - rect[1]);
    let matrix = match rot {
        90 => Some([0.0, 1.0, -1.0, 0.0, rw, 0.0]),
        180 => Some([-1.0, 0.0, 0.0, -1.0, rw, rh]),
        270 => Some([0.0, -1.0, 1.0, 0.0, 0.0, rh]),
        _ => None,
    };
    (rect, matrix, [0.0, 0.0, bbox_dims.0, bbox_dims.1])
}

fn page_annots(cx: &mut EditContext<'_>, page: &Object) -> papyrine_ops::Result<Object> {
    let doc = cx.doc();
    match cosx::get(page, "Annots").filter(|a| cosx::kind(a) == Some(ObjectKind::Array)) {
        Some(a) => {
            if a.is_indirect() {
                cx.touch(&a)?;
            } else {
                cx.touch(page)?;
            }
            Ok(a)
        }
        None => {
            cx.touch(page)?;
            let a = doc.new_array();
            page.dict_set("Annots", &a)?;
            Ok(a)
        }
    }
}

fn pdf_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let env = crate::engine::today();
    let t = env.today;
    let rem = secs % 86_400;
    format!(
        "D:{:04}{:02}{:02}{:02}{:02}{:02}Z",
        t.year,
        t.month,
        t.day,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

fn color_ops(c: &[f64]) -> String {
    match c.len() {
        1 => format!("{} g", c[0]),
        4 => format!("{} {} {} {} k", c[0], c[1], c[2], c[3]),
        _ => {
            let c = if c.len() == 3 { c } else { &[0.0, 0.0, 0.0] };
            format!("{} {} {} rg", c[0], c[1], c[2])
        }
    }
}

/// Build the annotation dictionary, install it in the page and return nothing; shared by the two
/// commands.
#[allow(clippy::too_many_arguments)]
fn add_freetext(
    cx: &mut EditContext<'_>,
    page_index: usize,
    rect: [f64; 4],
    ap: &Ap,
    matrix: Option<[f64; 6]>,
    bbox: [f64; 4],
    contents: &str,
    da: &str,
    mark: Option<&str>,
    font_obj: Option<Object>,
) -> Result<()> {
    let doc: &Document = cx.doc();
    let page = doc
        .page(page_index)
        .map_err(|_| Error::invalid(format!("page {page_index} out of range")))?;
    let mut ap = ap.clone();
    ap.bbox = bbox;
    ap.matrix = matrix;
    if let (Some(fu), Some(f)) = (ap.font.as_mut(), font_obj) {
        fu.obj = Some(f);
    }
    // The AP stream, via the same writer the field widgets use.
    let stream = install::standalone_xobject(cx, &ap)?;
    let doc = cx.doc();
    let annot = doc.new_dict();
    annot.dict_set("Type", &doc.new_name("Annot")?)?;
    annot.dict_set("Subtype", &doc.new_name("FreeText")?)?;
    let r = doc.new_array();
    for v in rect {
        r.array_push(&doc.new_real(v)?)?;
    }
    annot.dict_set("Rect", &r)?;
    annot.dict_set(
        "Contents",
        &doc.new_string(papyrine_ops::encode_text_string(contents))?,
    )?;
    annot.dict_set("DA", &doc.new_string(da)?)?;
    annot.dict_set("F", &doc.new_int(4))?;
    annot.dict_set("IT", &doc.new_name("FreeTextTypeWriter")?)?;
    let border = doc.new_array();
    for _ in 0..3 {
        border.array_push(&doc.new_int(0))?;
    }
    annot.dict_set("Border", &border)?;
    let bs = doc.new_dict();
    bs.dict_set("W", &doc.new_int(0))?;
    annot.dict_set("BS", &bs)?;
    annot.dict_set("M", &doc.new_string(pdf_date())?)?;
    annot.dict_set("CA", &doc.new_int(1))?;
    if let Some(m) = mark {
        annot.dict_set("Papyrine_Mark", &doc.new_name(m)?)?;
    }
    let apd = doc.new_dict();
    apd.dict_set("N", &stream)?;
    annot.dict_set("AP", &apd)?;
    let annot = doc.make_indirect(&annot)?;
    annot.dict_set("P", &page)?;
    let annots = page_annots(cx, &page)?;
    annots.array_push(&annot)?;
    cx.note_page(page_index);
    Ok(())
}

fn base14_font_object(cx: &EditContext<'_>, name: &str) -> Result<(Object, FieldFont)> {
    let doc = cx.doc();
    let d = doc.new_dict();
    d.dict_set("Type", &doc.new_name("Font")?)?;
    d.dict_set("Subtype", &doc.new_name("Type1")?)?;
    d.dict_set("BaseFont", &doc.new_name(name)?)?;
    d.dict_set("Encoding", &doc.new_name("WinAnsiEncoding")?)?;
    let f = font::from_font_object("Helv", &d).map_err(Error::from)?;
    Ok((d, f))
}

/// Click-to-type text on a page without form fields.
///
/// (`x`, `y`) is the displayed top-left corner of the text box in default user space (for an
/// unrotated page that is the usual PDF point; for a rotated page the caller passes the
/// corner as it appears on screen, mapped into user space). Text wraps at `width` when given.
/// Characters the font cannot show are an error (the UI should pick another font).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddFlatText {
    pub page: usize,
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub width: Option<f64>,
    pub text: String,
    #[serde(default = "default_size")]
    pub font_size: f64,
    /// Core 14 font name, default Helvetica.
    #[serde(default = "default_font")]
    pub font: String,
    /// Gray (1), RGB (3) or CMYK (4) components.
    #[serde(default)]
    pub color: Vec<f64>,
}

fn default_size() -> f64 {
    12.0
}
fn default_font() -> String {
    "Helvetica".into()
}

impl AddFlatText {
    pub const NAME: &'static str = "fill.add_text";

    pub fn new(page: usize, x: f64, y: f64, text: impl Into<String>) -> Self {
        AddFlatText {
            page,
            x,
            y,
            width: None,
            text: text.into(),
            font_size: 12.0,
            font: default_font(),
            color: vec![0.0],
        }
    }
}

impl Command for AddFlatText {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.fill.add-text").arg("page", self.page + 1)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        if self.text.is_empty() {
            return Err(Error::invalid("no text"));
        }
        if !(1.0..=500.0).contains(&self.font_size) {
            return Err(Error::invalid("font size out of range"));
        }
        let page = cx
            .doc()
            .page(self.page)
            .map_err(|_| Error::invalid(format!("page {} out of range", self.page)))?;
        let rot = rotation_of(&page);
        let (fobj, font) = base14_font_object(cx, &self.font)?;
        if !font.can_show(&self.text.replace(['\r', '\n'], " ")) {
            return Err(FillError::Appearance(
                "the text has characters the chosen font cannot show".into(),
            )
            .into());
        }
        let width = self.width.map(|w| w.max(self.font_size));
        let lay = appearance::free_text_ap(&self.text, &font, self.font_size, &self.color, width)
            .map_err(Error::from)?;
        let (rect, matrix, bbox) = place(rot, self.x, self.y, lay.width, lay.height);
        let da = format!("/Helv {} Tf {}", self.font_size, color_ops(&self.color));
        add_freetext(
            cx,
            self.page,
            rect,
            &lay.ap,
            matrix,
            bbox,
            &self.text,
            &da,
            None,
            Some(fobj),
        )?;
        cx.changeset()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkKind {
    Check,
    Cross,
    Dot,
}

impl MarkKind {
    fn code(self) -> u8 {
        match self {
            MarkKind::Check => b'4',
            MarkKind::Cross => b'8',
            MarkKind::Dot => b'l',
        }
    }
    fn contents(self) -> &'static str {
        match self {
            MarkKind::Check => "\u{2713}",
            MarkKind::Cross => "\u{2717}",
            MarkKind::Dot => "\u{2022}",
        }
    }
    fn tag(self) -> &'static str {
        match self {
            MarkKind::Check => "Check",
            MarkKind::Cross => "Cross",
            MarkKind::Dot => "Dot",
        }
    }
}

/// A check, cross or bullet mark centred on (`x`, `y`) in default user space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddMark {
    pub page: usize,
    pub kind: MarkKind,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_size")]
    pub size: f64,
    #[serde(default)]
    pub color: Vec<f64>,
}

impl AddMark {
    pub const NAME: &'static str = "fill.add_mark";

    pub fn new(page: usize, kind: MarkKind, x: f64, y: f64) -> Self {
        AddMark {
            page,
            kind,
            x,
            y,
            size: 12.0,
            color: vec![0.0],
        }
    }
}

impl Command for AddMark {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.fill.add-mark")
            .arg("kind", self.kind.tag())
            .arg("page", self.page + 1)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        if !(1.0..=500.0).contains(&self.size) {
            return Err(Error::invalid("mark size out of range"));
        }
        let page = cx
            .doc()
            .page(self.page)
            .map_err(|_| Error::invalid(format!("page {} out of range", self.page)))?;
        let rot = rotation_of(&page);
        let s = self.size;
        let ap = appearance::mark_ap(self.kind.code(), s, &self.color);
        let rect = [
            self.x - s / 2.0,
            self.y - s / 2.0,
            self.x + s / 2.0,
            self.y + s / 2.0,
        ];
        // A square box: only the matrix depends on the page rotation.
        let (_, matrix, _) = place(rot, 0.0, 0.0, s, s);
        let da = format!("/ZaDb {} Tf {}", s, color_ops(&self.color));
        add_freetext(
            cx,
            self.page,
            rect,
            &ap,
            matrix,
            [0.0, 0.0, s, s],
            self.kind.contents(),
            &da,
            Some(self.kind.tag()),
            None,
        )?;
        cx.changeset()
    }
}
