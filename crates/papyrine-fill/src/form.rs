//! A read-only view of the AcroForm over the COS objects: terminal fields with their inherited
//! attributes and widgets. It is rebuilt for every command (a few milliseconds for a 1,000
//! widget IRS form), so commands never see stale data and need no invalidation protocol.

use std::collections::HashSet;

use papyrine_cos::{DecodeLevel, Document, ObjId, Object, ObjectKind};

use crate::cosx::{self, MAX_DEPTH};
use crate::error::{FillError, Result};

pub mod flags {
    pub const READ_ONLY: u32 = 1;
    pub const REQUIRED: u32 = 1 << 1;
    pub const MULTILINE: u32 = 1 << 12;
    pub const PASSWORD: u32 = 1 << 13;
    pub const NO_TOGGLE_TO_OFF: u32 = 1 << 14;
    pub const RADIO: u32 = 1 << 15;
    pub const PUSH_BUTTON: u32 = 1 << 16;
    pub const COMBO: u32 = 1 << 17;
    pub const EDIT: u32 = 1 << 18;
    pub const MULTI_SELECT: u32 = 1 << 21;
    pub const DO_NOT_SCROLL: u32 = 1 << 23;
    pub const COMB: u32 = 1 << 24;
    pub const RICH_TEXT: u32 = 1 << 25;
    pub const RADIOS_IN_UNISON: u32 = 1 << 25;
    pub const FILE_SELECT: u32 = 1 << 20;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Checkbox,
    Radio,
    PushButton,
    Combo,
    List,
    Signature,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XfaState {
    None,
    /// Hybrid form: the AcroForm is the live form and XFA is a duplicate.
    Static,
    /// The AcroForm is a placeholder; viewers render from the XFA template.
    Dynamic,
}

/// A field value as stored in `/V`.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum RawValue {
    #[default]
    None,
    Text(String),
    Name(String),
    List(Vec<String>),
}

impl RawValue {
    /// The single text of the value (first element of a list).
    pub fn as_text(&self) -> Option<&str> {
        match self {
            RawValue::Text(s) | RawValue::Name(s) => Some(s),
            RawValue::List(l) => l.first().map(String::as_str),
            RawValue::None => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorderStyle {
    Solid,
    Dashed,
    Beveled,
    Inset,
    Underline,
}

#[derive(Clone)]
pub struct Widget {
    pub obj: Object,
    pub id: Option<ObjId>,
    /// Normalised `[x0 y0 x1 y1]`.
    pub rect: [f64; 4],
    /// `/MK /R`, normalised to 0, 90, 180 or 270.
    pub rotation: u16,
    pub background: Option<Vec<f64>>,
    pub border_color: Option<Vec<f64>>,
    /// `/MK /CA`.
    pub caption: Option<String>,
    pub border_width: f64,
    pub border_style: BorderStyle,
    pub dash: Vec<f64>,
    /// `/AS`.
    pub appearance_state: Option<String>,
    /// The non-`Off` key of `/AP /N` (or `/AP /D`), for checkboxes and radios.
    pub on_state: Option<String>,
    /// Keys of the `/AP /N` dictionary (empty when `/N` is a stream or missing).
    pub ap_states: Vec<String>,
    pub has_normal_ap: bool,
    /// Annotation flags `/F`.
    pub annot_flags: u32,
}

impl Widget {
    pub fn width(&self) -> f64 {
        self.rect[2] - self.rect[0]
    }
    pub fn height(&self) -> f64 {
        self.rect[3] - self.rect[1]
    }
}

#[derive(Clone)]
pub struct Field {
    pub obj: Object,
    pub id: ObjId,
    pub name: String,
    pub kind: Kind,
    pub flags: u32,
    pub value: RawValue,
    pub default_value: RawValue,
    pub da: Option<String>,
    pub q: i64,
    pub max_len: Option<u32>,
    /// `/Opt` as (export value, display text).
    pub options: Vec<(String, String)>,
    pub top_index: Option<u32>,
    pub selected_indices: Vec<u32>,
    pub widgets: Vec<Widget>,
}

impl Field {
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
    pub fn is_read_only(&self) -> bool {
        self.has(flags::READ_ONLY)
    }
    pub fn multiline(&self) -> bool {
        self.kind == Kind::Text && self.has(flags::MULTILINE)
    }
    pub fn password(&self) -> bool {
        self.kind == Kind::Text && self.has(flags::PASSWORD)
    }
    pub fn comb(&self) -> bool {
        self.kind == Kind::Text
            && self.has(flags::COMB)
            && self.max_len.is_some()
            && !self.has(flags::MULTILINE | flags::PASSWORD | flags::FILE_SELECT)
    }
    pub fn editable_combo(&self) -> bool {
        self.kind == Kind::Combo && self.has(flags::EDIT)
    }
    pub fn multi_select(&self) -> bool {
        self.kind == Kind::List && self.has(flags::MULTI_SELECT)
    }
    /// Text kinds whose appearance shows a string.
    pub fn shows_text(&self) -> bool {
        matches!(self.kind, Kind::Text | Kind::Combo | Kind::List)
    }
}

pub struct FormTree {
    pub acro: Option<Object>,
    pub fields: Vec<Field>,
    /// `/CO`, as object ids.
    pub calc_order: Vec<ObjId>,
    pub need_appearances: bool,
    pub da: Option<String>,
    pub q: i64,
    pub dr: Option<Object>,
    pub xfa: XfaState,
    pub truncated: bool,
}

const MAX_FIELDS: usize = 200_000;

impl FormTree {
    pub fn load(doc: &Document) -> Result<FormTree> {
        let mut t = FormTree {
            acro: None,
            fields: Vec::new(),
            calc_order: Vec::new(),
            need_appearances: false,
            da: None,
            q: 0,
            dr: None,
            xfa: XfaState::None,
            truncated: false,
        };
        let root = doc.root()?;
        let Some(acro) = cosx::get(&root, "AcroForm").filter(cosx::is_dict) else {
            return Ok(t);
        };
        t.need_appearances = cosx::get_bool(&acro, "NeedAppearances").unwrap_or(false);
        t.da = cosx::get_text(&acro, "DA");
        t.q = cosx::get_int(&acro, "Q").unwrap_or(0).clamp(0, 2);
        t.dr = cosx::get(&acro, "DR");
        let needs_rendering = cosx::get_bool(&root, "NeedsRendering").unwrap_or(false);
        t.xfa = xfa_state(&acro, needs_rendering);
        let mut w = Walker {
            seen: HashSet::new(),
            out: Vec::new(),
            da: t.da.clone(),
            q: t.q,
            truncated: false,
        };
        let top = cosx::get(&acro, "Fields")
            .map(|f| cosx::items(&f))
            .unwrap_or_default();
        for f in top {
            w.visit(&f, "", 0);
        }
        t.fields = w.out;
        t.truncated = w.truncated;
        t.calc_order = cosx::get(&acro, "CO")
            .map(|c| cosx::items(&c))
            .unwrap_or_default()
            .iter()
            .filter_map(Object::id)
            .collect();
        t.acro = Some(acro);
        Ok(t)
    }

    pub fn has_form(&self) -> bool {
        self.acro.is_some()
    }

    pub fn by_id(&self, id: ObjId) -> Option<&Field> {
        self.fields.iter().find(|f| f.id == id)
    }

    pub fn by_name(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name == name)
    }

    pub fn index_of(&self, id: ObjId) -> Option<usize> {
        self.fields.iter().position(|f| f.id == id)
    }

    pub fn resolve(&self, r: &FieldRef) -> Result<&Field> {
        match r {
            FieldRef::Name(n) => self.by_name(n),
            FieldRef::Id { num, generation } => self.by_id(ObjId::new(*num, *generation)),
        }
        .ok_or_else(|| FillError::NoSuchField(r.to_string()))
    }
}

/// How a command names a field: its fully qualified name, or its object id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum FieldRef {
    Name(String),
    Id { num: u32, generation: u16 },
}

impl FieldRef {
    pub fn id(id: ObjId) -> Self {
        FieldRef::Id {
            num: id.num,
            generation: id.generation,
        }
    }
}

impl From<&str> for FieldRef {
    fn from(s: &str) -> Self {
        FieldRef::Name(s.to_string())
    }
}

impl std::fmt::Display for FieldRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FieldRef::Name(n) => f.write_str(n),
            FieldRef::Id { num, generation } => write!(f, "{num} {generation} R"),
        }
    }
}

struct Walker {
    seen: HashSet<ObjId>,
    out: Vec<Field>,
    da: Option<String>,
    q: i64,
    truncated: bool,
}

impl Walker {
    fn visit(&mut self, node: &Object, parent_name: &str, depth: usize) {
        if !cosx::is_dict(node) || depth > MAX_DEPTH {
            self.truncated |= depth > MAX_DEPTH;
            return;
        }
        if self.out.len() >= MAX_FIELDS {
            self.truncated = true;
            return;
        }
        let Some(id) = node.id() else {
            // A direct field dictionary cannot be edited in place by id; skip it.
            return;
        };
        if !self.seen.insert(id) {
            return;
        }
        let partial = cosx::get_text(node, "T").unwrap_or_default();
        let name = match (parent_name.is_empty(), partial.is_empty()) {
            (_, true) => parent_name.to_string(),
            (true, false) => partial,
            (false, false) => format!("{parent_name}.{partial}"),
        };
        let kids: Vec<Object> = cosx::get(node, "Kids")
            .map(|k| cosx::items(&k))
            .unwrap_or_default()
            .into_iter()
            .filter(cosx::is_dict)
            .collect();
        let (field_kids, widget_kids): (Vec<Object>, Vec<Object>) =
            kids.into_iter().partition(|k| cosx::get(k, "T").is_some());
        if !field_kids.is_empty() {
            for k in &field_kids {
                self.visit(k, &name, depth + 1);
            }
            return;
        }
        let widget_objs = if widget_kids.is_empty() {
            if cosx::get_name(node, "Subtype").as_deref() == Some("Widget")
                || cosx::get(node, "Rect").is_some()
            {
                vec![node.clone()]
            } else {
                Vec::new()
            }
        } else {
            widget_kids
        };
        let field = self.make_field(node, id, name, &widget_objs);
        self.out.push(field);
    }

    fn make_field(&self, node: &Object, id: ObjId, name: String, widgets: &[Object]) -> Field {
        let fl = cosx::inherited(node, "Ff")
            .as_ref()
            .and_then(cosx::int)
            .unwrap_or(0) as u32;
        let ft = cosx::inherited(node, "FT").as_ref().and_then(cosx::name);
        let kind = match ft.as_deref() {
            Some("Tx") => Kind::Text,
            Some("Btn") => {
                if fl & flags::PUSH_BUTTON != 0 {
                    Kind::PushButton
                } else if fl & flags::RADIO != 0 {
                    Kind::Radio
                } else {
                    Kind::Checkbox
                }
            }
            Some("Ch") => {
                if fl & flags::COMBO != 0 {
                    Kind::Combo
                } else {
                    Kind::List
                }
            }
            Some("Sig") => Kind::Signature,
            _ => Kind::Other,
        };
        let value =
            |key: &str| cosx::inherited(node, key).map_or(RawValue::None, |v| raw_value(&v));
        let options = cosx::inherited(node, "Opt")
            .map(|o| {
                cosx::items(&o)
                    .iter()
                    .filter_map(|e| {
                        if let Some(s) = cosx::text(e) {
                            return Some((s.clone(), s));
                        }
                        let pair = cosx::items(e);
                        (pair.len() >= 2).then(|| {
                            (
                                cosx::text(&pair[0]).unwrap_or_default(),
                                cosx::text(&pair[1]).unwrap_or_default(),
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Field {
            obj: node.clone(),
            id,
            name,
            kind,
            flags: fl,
            value: value("V"),
            default_value: value("DV"),
            da: cosx::inherited(node, "DA")
                .as_ref()
                .and_then(cosx::text)
                .or_else(|| self.da.clone()),
            q: cosx::inherited(node, "Q")
                .as_ref()
                .and_then(cosx::int)
                .map_or(self.q, |q| q.clamp(0, 2)),
            max_len: cosx::inherited(node, "MaxLen")
                .as_ref()
                .and_then(cosx::int)
                .filter(|&n| n > 0)
                .map(|n| n.min(i64::from(u32::MAX)) as u32),
            options,
            top_index: cosx::inherited(node, "TI")
                .as_ref()
                .and_then(cosx::int)
                .filter(|&n| n >= 0)
                .map(|n| n as u32),
            selected_indices: cosx::inherited(node, "I")
                .map(|a| {
                    cosx::items(&a)
                        .iter()
                        .filter_map(cosx::int)
                        .filter(|&i| i >= 0)
                        .map(|i| i as u32)
                        .collect()
                })
                .unwrap_or_default(),
            widgets: widgets.iter().filter_map(make_widget).collect(),
        }
    }
}

fn raw_value(v: &Object) -> RawValue {
    match cosx::kind(v) {
        Some(ObjectKind::String | ObjectKind::Stream) => {
            RawValue::Text(cosx::text(v).unwrap_or_default())
        }
        Some(ObjectKind::Name) => RawValue::Name(cosx::name(v).unwrap_or_default()),
        Some(ObjectKind::Array) => RawValue::List(
            cosx::items(v)
                .iter()
                .filter_map(|e| cosx::text(e).or_else(|| cosx::name(e)))
                .collect(),
        ),
        _ => RawValue::None,
    }
}

fn make_widget(w: &Object) -> Option<Widget> {
    let rect = cosx::get(w, "Rect")
        .and_then(|r| cosx::rect(&r))
        .unwrap_or([0.0; 4]);
    let ap = cosx::get(w, "AP");
    let ap_n = ap.as_ref().and_then(|a| cosx::get(a, "N"));
    let has_normal_ap = ap_n.is_some();
    let ap_states: Vec<String> = match &ap_n {
        Some(n) if cosx::kind(n) == Some(ObjectKind::Dictionary) => n
            .dict_keys()
            .unwrap_or_default()
            .into_iter()
            .map(|k| String::from_utf8_lossy(&k).into_owned())
            .collect(),
        _ => Vec::new(),
    };
    let on_state = ap_states
        .iter()
        .find(|s| s.as_str() != "Off")
        .cloned()
        .or_else(|| {
            let d = ap.as_ref().and_then(|a| cosx::get(a, "D"))?;
            if cosx::kind(&d) != Some(ObjectKind::Dictionary) {
                return None;
            }
            d.dict_keys()
                .ok()?
                .into_iter()
                .map(|k| String::from_utf8_lossy(&k).into_owned())
                .find(|s| s != "Off")
        });
    let mk = cosx::get(w, "MK");
    let color = |key: &str| -> Option<Vec<f64>> {
        let c = cosx::numbers(&mk.as_ref().and_then(|d| cosx::get(d, key))?);
        (!c.is_empty()).then_some(c)
    };
    let rotation = mk
        .as_ref()
        .and_then(|d| cosx::get_int(d, "R"))
        .map_or(0, |r| (r.rem_euclid(360) / 90 * 90) as u16);
    let (mut border_width, mut border_style, mut dash) = (1.0, BorderStyle::Solid, Vec::new());
    if let Some(bs) = cosx::get(w, "BS") {
        if let Some(wd) = cosx::get_num(&bs, "W") {
            border_width = wd.max(0.0);
        }
        border_style = match cosx::get_name(&bs, "S").as_deref() {
            Some("D") => BorderStyle::Dashed,
            Some("B") => BorderStyle::Beveled,
            Some("I") => BorderStyle::Inset,
            Some("U") => BorderStyle::Underline,
            _ => BorderStyle::Solid,
        };
        dash = cosx::get(&bs, "D")
            .map(|d| cosx::numbers(&d))
            .unwrap_or_default();
    } else if let Some(b) = cosx::get(w, "Border") {
        let n = cosx::numbers(&b);
        if n.len() >= 3 {
            border_width = n[2].max(0.0);
        }
        if let Some(d) = cosx::items(&b).get(3) {
            dash = cosx::numbers(d);
            if !dash.is_empty() {
                border_style = BorderStyle::Dashed;
            }
        }
    }
    Some(Widget {
        obj: w.clone(),
        id: w.id(),
        rect,
        rotation,
        background: color("BG"),
        border_color: color("BC"),
        caption: mk.as_ref().and_then(|d| cosx::get_text(d, "CA")),
        border_width,
        border_style,
        dash,
        appearance_state: cosx::get_name(w, "AS"),
        on_state,
        ap_states,
        has_normal_ap,
        annot_flags: cosx::get_int(w, "F").unwrap_or(0) as u32,
    })
}

fn xfa_state(acro: &Object, needs_rendering: bool) -> XfaState {
    let Some(xfa) = cosx::get(acro, "XFA") else {
        return XfaState::None;
    };
    let template = match cosx::kind(&xfa) {
        Some(ObjectKind::Array) => cosx::items(&xfa)
            .as_chunks::<2>()
            .0
            .iter()
            .find(|p| cosx::text(&p[0]).as_deref() == Some("template"))
            .map(|p| p[1].clone()),
        Some(ObjectKind::Stream) => Some(xfa.clone()),
        _ => None,
    };
    let Some(t) = template else {
        return if needs_rendering {
            XfaState::Dynamic
        } else {
            XfaState::Static
        };
    };
    let Ok(data) = t.stream_decoded(DecodeLevel::Generalized) else {
        return XfaState::Dynamic;
    };
    let bytes = data.as_slice();
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    let Some(pos) = find(bytes, b"<template") else {
        return XfaState::Dynamic;
    };
    let head = &bytes[pos..bytes.len().min(pos + 1024)];
    let head = &head[..find(head, b">").unwrap_or(head.len())];
    if find(head, b"interactiveForms").is_some() {
        XfaState::Static
    } else {
        XfaState::Dynamic
    }
}
