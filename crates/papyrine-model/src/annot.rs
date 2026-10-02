//! Annotations per page.

use std::collections::HashSet;
use std::rc::Rc;

use papyrine_core::geom::{Point, Rect};
use papyrine_cos::{ObjId, Object, ObjectKind, Result};

use crate::dest::{Action, ActionKind, Destination};
use crate::model::{Key, MAX_NODES, Model};
use crate::text::PdfDate;

pub mod annot_flags {
    pub const INVISIBLE: u32 = 1;
    pub const HIDDEN: u32 = 1 << 1;
    pub const PRINT: u32 = 1 << 2;
    pub const NO_ZOOM: u32 = 1 << 3;
    pub const NO_ROTATE: u32 = 1 << 4;
    pub const NO_VIEW: u32 = 1 << 5;
    pub const READ_ONLY: u32 = 1 << 6;
    pub const LOCKED: u32 = 1 << 7;
    pub const TOGGLE_NO_VIEW: u32 = 1 << 8;
    pub const LOCKED_CONTENTS: u32 = 1 << 9;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderKind {
    Solid,
    Dashed,
    Beveled,
    Inset,
    Underline,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BorderStyle {
    pub width: f64,
    pub style: BorderKind,
    pub dash: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AnnotSubtype {
    Text,
    Link,
    FreeText,
    Line,
    Square,
    Circle,
    Polygon,
    PolyLine,
    Highlight,
    Underline,
    Squiggly,
    StrikeOut,
    Stamp,
    Caret,
    Ink,
    Popup,
    FileAttachment,
    Sound,
    Movie,
    Widget,
    Screen,
    PrinterMark,
    TrapNet,
    Watermark,
    ThreeD,
    Redact,
    RichMedia,
    Other(String),
}

impl AnnotSubtype {
    pub fn from_name(n: &str) -> AnnotSubtype {
        use AnnotSubtype::*;
        match n {
            "Text" => Text,
            "Link" => Link,
            "FreeText" => FreeText,
            "Line" => Line,
            "Square" => Square,
            "Circle" => Circle,
            "Polygon" => Polygon,
            "PolyLine" => PolyLine,
            "Highlight" => Highlight,
            "Underline" => Underline,
            "Squiggly" => Squiggly,
            "StrikeOut" => StrikeOut,
            "Stamp" => Stamp,
            "Caret" => Caret,
            "Ink" => Ink,
            "Popup" => Popup,
            "FileAttachment" => FileAttachment,
            "Sound" => Sound,
            "Movie" => Movie,
            "Widget" => Widget,
            "Screen" => Screen,
            "PrinterMark" => PrinterMark,
            "TrapNet" => TrapNet,
            "Watermark" => Watermark,
            "3D" => ThreeD,
            "Redact" => Redact,
            "RichMedia" => RichMedia,
            other => Other(other.to_string()),
        }
    }

    pub fn name(&self) -> &str {
        use AnnotSubtype::*;
        match self {
            Text => "Text",
            Link => "Link",
            FreeText => "FreeText",
            Line => "Line",
            Square => "Square",
            Circle => "Circle",
            Polygon => "Polygon",
            PolyLine => "PolyLine",
            Highlight => "Highlight",
            Underline => "Underline",
            Squiggly => "Squiggly",
            StrikeOut => "StrikeOut",
            Stamp => "Stamp",
            Caret => "Caret",
            Ink => "Ink",
            Popup => "Popup",
            FileAttachment => "FileAttachment",
            Sound => "Sound",
            Movie => "Movie",
            Widget => "Widget",
            Screen => "Screen",
            PrinterMark => "PrinterMark",
            TrapNet => "TrapNet",
            Watermark => "Watermark",
            ThreeD => "3D",
            Redact => "Redact",
            RichMedia => "RichMedia",
            Other(s) => s,
        }
    }

    pub fn is_text_markup(&self) -> bool {
        matches!(
            self,
            AnnotSubtype::Highlight
                | AnnotSubtype::Underline
                | AnnotSubtype::Squiggly
                | AnnotSubtype::StrikeOut
        )
    }
}

#[derive(Debug, Clone)]
pub struct Annotation {
    /// `None` for a direct (inline) annotation dictionary.
    pub id: Option<ObjId>,
    pub page: usize,
    /// Position in the page's `/Annots` array.
    pub index: usize,
    pub subtype: AnnotSubtype,
    pub rect: Rect,
    pub flags: u32,
    /// `/Contents`.
    pub contents: Option<String>,
    /// `/T`, the author.
    pub title: Option<String>,
    pub subject: Option<String>,
    /// `/NM`.
    pub name: Option<String>,
    pub modified: Option<PdfDate>,
    /// `/M` as written (it may be free text rather than a date).
    pub modified_raw: Option<String>,
    pub creation_date: Option<PdfDate>,
    /// `/C` components (0 = transparent, 1 gray, 3 RGB, 4 CMYK).
    pub color: Option<Vec<f64>>,
    /// `/IC` interior colour.
    pub interior_color: Option<Vec<f64>>,
    /// `/CA` constant opacity (default 1).
    pub opacity: f64,
    pub border: Option<BorderStyle>,
    /// `/QuadPoints`, one `[p1 p2 p3 p4]` per quad.
    pub quad_points: Vec<[Point; 4]>,
    pub ink_list: Vec<Vec<Point>>,
    /// `/L` endpoints of a Line.
    pub line: Option<[Point; 2]>,
    pub line_endings: Vec<String>,
    pub vertices: Vec<Point>,
    /// `/IRT` (the annotation this one replies to) and `/RT` (`R` or `Group`).
    pub in_reply_to: Option<ObjId>,
    pub reply_type: Option<String>,
    /// For Text / markup annotations: the associated Popup. For a Popup: its parent.
    pub popup: Option<ObjId>,
    pub parent: Option<ObjId>,
    /// Popup / Text `/Open`.
    pub open: Option<bool>,
    /// `/Name` (icon name, stamp name).
    pub icon_name: Option<String>,
    /// `/IT` intent.
    pub intent: Option<String>,
    /// Review state model and state (`/StateModel`, `/State`).
    pub state_model: Option<String>,
    pub state: Option<String>,
    /// `/DA` and `/Q` of a FreeText annotation.
    pub default_appearance: Option<String>,
    pub quadding: Option<u8>,
    pub has_appearance: bool,
    pub ap_states: Vec<String>,
    /// `/A` (Link and others), `/Dest` resolved.
    pub action: Option<Action>,
    pub dest: Option<Destination>,
    /// File name of a FileAttachment's file specification.
    pub file_name: Option<String>,
}

impl Annotation {
    pub fn has_flag(&self, f: u32) -> bool {
        self.flags & f != 0
    }
    /// The URI of a Link annotation with a URI action.
    pub fn uri(&self) -> Option<&str> {
        match &self.action.as_ref()?.kind {
            ActionKind::Uri { uri, .. } => Some(uri),
            _ => None,
        }
    }
}

/// All annotations of one page, in `/Annots` order.
#[derive(Debug, Default)]
pub struct AnnotationSet {
    pub page: usize,
    pub items: Vec<Annotation>,
}

impl AnnotationSet {
    pub fn by_id(&self, id: ObjId) -> Option<&Annotation> {
        self.items.iter().find(|a| a.id == Some(id))
    }
    /// Direct replies (`/IRT`) to an annotation.
    pub fn replies_to(&self, id: ObjId) -> Vec<&Annotation> {
        self.items
            .iter()
            .filter(|a| a.in_reply_to == Some(id))
            .collect()
    }
    /// The Popup annotation attached to `id`, if present on the page.
    pub fn popup_of(&self, id: ObjId) -> Option<&Annotation> {
        let a = self.by_id(id)?;
        a.popup.and_then(|p| self.by_id(p)).or_else(|| {
            self.items
                .iter()
                .find(|p| p.subtype == AnnotSubtype::Popup && p.parent == Some(id))
        })
    }
    pub fn of_subtype<'a>(
        &'a self,
        s: &'a AnnotSubtype,
    ) -> impl Iterator<Item = &'a Annotation> + 'a {
        self.items.iter().filter(move |a| &a.subtype == s)
    }
}

impl Model {
    /// Annotations of page `index`, materialized on first use.
    pub fn annotations(&self, index: usize) -> Result<Rc<AnnotationSet>> {
        self.cached(Key::Annots(index), |m| {
            m.dep_id(crate::model::PAGE_TREE);
            let page = m.page_object(index)?;
            let mut set = AnnotationSet {
                page: index,
                items: Vec::new(),
            };
            let mut seen = HashSet::new();
            for (i, a) in m.items_of(&page, "Annots").into_iter().enumerate() {
                if set.items.len() >= MAX_NODES || !m.is_dict(&a) {
                    continue;
                }
                if let Some(id) = a.id()
                    && !seen.insert(id)
                {
                    continue; // the same annotation listed twice
                }
                set.items.push(m.build_annotation(index, i, &a));
            }
            Ok(set)
        })
    }

    pub(crate) fn border_style(&self, a: &Object) -> Option<BorderStyle> {
        if let Some(bs) = self.get(a, "BS") {
            let style = match self.name_of(&bs, "S").as_deref() {
                Some("D") => BorderKind::Dashed,
                Some("B") => BorderKind::Beveled,
                Some("I") => BorderKind::Inset,
                Some("U") => BorderKind::Underline,
                _ => BorderKind::Solid,
            };
            return Some(BorderStyle {
                width: self.num_of(&bs, "W").unwrap_or(1.0),
                style,
                dash: self
                    .get(&bs, "D")
                    .map(|d| self.numbers(&d))
                    .unwrap_or_else(|| vec![3.0]),
            });
        }
        let b = self.numbers(&self.get(a, "Border")?);
        (b.len() >= 3).then(|| {
            let dash = self
                .get(a, "Border")
                .and_then(|arr| self.items(&arr).get(3).cloned())
                .map(|d| self.numbers(&d))
                .unwrap_or_default();
            BorderStyle {
                width: b[2],
                style: if dash.is_empty() {
                    BorderKind::Solid
                } else {
                    BorderKind::Dashed
                },
                dash,
            }
        })
    }

    fn points(&self, nums: &[f64]) -> Vec<Point> {
        nums.as_chunks::<2>()
            .0
            .iter()
            .map(|c| Point::new(c[0], c[1]))
            .collect()
    }

    fn build_annotation(&self, page: usize, index: usize, a: &Object) -> Annotation {
        let subtype = AnnotSubtype::from_name(&self.name_of(a, "Subtype").unwrap_or_default());
        let date = |key: &str| self.text_of(a, key);
        let modified_raw = date("M");
        let ap_n = self.get(a, "AP").and_then(|ap| self.get(&ap, "N"));
        let ap_states = match &ap_n {
            Some(n) if self.kind(n) == ObjectKind::Dictionary => self.dict_keys(n),
            _ => Vec::new(),
        };
        let color = |key: &str| {
            let c = self.numbers_of(a, key);
            self.get(a, key).map(|_| c)
        };
        let quad_points = self
            .numbers_of(a, "QuadPoints")
            .as_chunks::<8>()
            .0
            .iter()
            .map(|q| {
                [
                    Point::new(q[0], q[1]),
                    Point::new(q[2], q[3]),
                    Point::new(q[4], q[5]),
                    Point::new(q[6], q[7]),
                ]
            })
            .collect();
        let ink_list = self
            .items_of(a, "InkList")
            .iter()
            .map(|s| self.points(&self.numbers(s)))
            .collect();
        let line = {
            let l = self.numbers_of(a, "L");
            (l.len() >= 4).then(|| [Point::new(l[0], l[1]), Point::new(l[2], l[3])])
        };
        let action = self.get(a, "A").and_then(|x| self.parse_action(&x));
        let dest = self
            .get(a, "Dest")
            .and_then(|d| self.parse_destination(&d))
            .or_else(|| action.as_ref().and_then(|x| x.goto_destination().cloned()));
        Annotation {
            id: a.id(),
            page,
            index,
            subtype,
            rect: self.rect_of(a, "Rect").unwrap_or_default(),
            flags: self.int_of(a, "F").unwrap_or(0) as u32,
            contents: self.text_of(a, "Contents"),
            title: self.text_of(a, "T"),
            subject: self.text_of(a, "Subj"),
            name: self.text_of(a, "NM"),
            modified: modified_raw.as_deref().and_then(PdfDate::parse),
            modified_raw,
            creation_date: date("CreationDate").as_deref().and_then(PdfDate::parse),
            color: color("C"),
            interior_color: color("IC"),
            opacity: self.num_of(a, "CA").unwrap_or(1.0),
            border: self.border_style(a),
            quad_points,
            ink_list,
            line,
            line_endings: self
                .items_of(a, "LE")
                .iter()
                .filter_map(|e| self.name(e))
                .collect(),
            vertices: self.points(&self.numbers_of(a, "Vertices")),
            in_reply_to: self.id_of(a, "IRT"),
            reply_type: self.name_of(a, "RT"),
            popup: self.id_of(a, "Popup"),
            parent: self.id_of(a, "Parent"),
            open: self.bool_of(a, "Open"),
            icon_name: self.name_of(a, "Name"),
            intent: self.name_of(a, "IT"),
            state_model: self.text_of(a, "StateModel"),
            state: self.text_of(a, "State"),
            default_appearance: self.get(a, "DA").and_then(|d| self.text(&d)),
            quadding: self.int_of(a, "Q").map(|q| q.clamp(0, 2) as u8),
            has_appearance: ap_n.is_some(),
            ap_states,
            action,
            dest,
            file_name: self.get(a, "FS").and_then(|f| self.file_spec(&f)),
        }
    }
}
