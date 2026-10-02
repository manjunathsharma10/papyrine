//! AcroForm: the full field tree, widgets, actions and calculation order.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use papyrine_core::geom::Rect;
use papyrine_cos::{DecodeLevel, ObjId, Object, ObjectKind, Result};

use crate::annot::BorderStyle;
use crate::dest::Action;
use crate::model::{Key, MAX_DEPTH, MAX_NODES, Model};
use crate::pages::normalize_rotation;

pub mod flags {
    pub const READ_ONLY: u32 = 1;
    pub const REQUIRED: u32 = 1 << 1;
    pub const NO_EXPORT: u32 = 1 << 2;
    // Text
    pub const MULTILINE: u32 = 1 << 12;
    pub const PASSWORD: u32 = 1 << 13;
    pub const FILE_SELECT: u32 = 1 << 20;
    pub const DO_NOT_SPELL_CHECK: u32 = 1 << 22;
    pub const DO_NOT_SCROLL: u32 = 1 << 23;
    pub const COMB: u32 = 1 << 24;
    pub const RICH_TEXT: u32 = 1 << 25;
    // Button
    pub const NO_TOGGLE_TO_OFF: u32 = 1 << 14;
    pub const RADIO: u32 = 1 << 15;
    pub const PUSH_BUTTON: u32 = 1 << 16;
    pub const RADIOS_IN_UNISON: u32 = 1 << 25;
    // Choice
    pub const COMBO: u32 = 1 << 17;
    pub const EDIT: u32 = 1 << 18;
    pub const SORT: u32 = 1 << 19;
    pub const MULTI_SELECT: u32 = 1 << 21;
    pub const COMMIT_ON_SEL_CHANGE: u32 = 1 << 26;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    PushButton,
    Checkbox,
    Radio,
    ComboBox,
    ListBox,
    Signature,
    /// `/FT` missing or unrecognised; usually a non-terminal grouping node.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XfaKind {
    None,
    /// XFA data with the `interactiveForms` base profile: the AcroForm fields are the live form.
    Static,
    /// Dynamic XFA: the AcroForm is a placeholder and viewers render from the XFA template.
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum FieldValue {
    #[default]
    None,
    Text(String),
    /// Checkbox / radio state or other name value.
    Name(String),
    List(Vec<String>),
}

impl FieldValue {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            FieldValue::Text(s) | FieldValue::Name(s) => Some(s),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChoiceOption {
    pub export: String,
    pub display: String,
}

#[derive(Debug, Clone)]
pub struct FieldAction {
    /// `K` keystroke, `F` format, `V` validate, `C` calculate, or `E X D U Fo Bl PO PC PV PI`.
    pub trigger: String,
    pub action: Action,
}

#[derive(Debug, Clone)]
pub struct Widget {
    pub id: ObjId,
    pub rect: Rect,
    /// Page index, from the page's `/Annots` (preferred) or the widget's `/P`.
    pub page: Option<usize>,
    /// Annotation flags `/F`.
    pub flags: u32,
    /// `/AS`.
    pub appearance_state: Option<String>,
    /// For checkboxes and radios: the non-`Off` state name in the normal appearance.
    pub on_state: Option<String>,
    /// Keys of the normal appearance subdictionary.
    pub ap_states: Vec<String>,
    pub has_appearance: bool,
    /// Checkbox / radio widget currently on.
    pub is_on: bool,
    /// `/MK /BC` and `/MK /BG` components.
    pub border_color: Option<Vec<f64>>,
    pub background: Option<Vec<f64>>,
    /// `/MK /CA` button caption.
    pub caption: Option<String>,
    pub rotation: u16,
    pub border: Option<BorderStyle>,
    /// `/H` highlighting mode letter.
    pub highlight: Option<char>,
    /// `/A` (mouse-up) action.
    pub action: Option<Action>,
}

#[derive(Debug, Clone)]
pub struct Field {
    pub id: ObjId,
    /// `/T`, the partial name.
    pub name: String,
    /// Dot-joined partial names from the root.
    pub qualified_name: String,
    pub kind: FieldKind,
    /// `/Ff`, inherited.
    pub flags: u32,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Terminal fields carry the value and own the widgets.
    pub is_terminal: bool,
    pub value: FieldValue,
    pub default_value: FieldValue,
    /// `/DA`, inherited, falling back to the AcroForm's.
    pub default_appearance: Option<String>,
    /// `/Q`: 0 left, 1 centre, 2 right.
    pub quadding: u8,
    pub max_len: Option<u32>,
    pub options: Vec<ChoiceOption>,
    pub top_index: Option<u32>,
    /// `/I` selected option indices (multi-select lists).
    pub selected_indices: Vec<u32>,
    /// `/TU` tooltip and `/TM` mapping name.
    pub tooltip: Option<String>,
    pub mapping_name: Option<String>,
    /// Checkbox / radio: the on-state name of each widget, in widget order.
    pub export_values: Vec<String>,
    pub actions: Vec<FieldAction>,
    pub widgets: Vec<Widget>,
    /// A widget found on a page but absent from the `/AcroForm /Fields` tree.
    pub orphan: bool,
}

impl Field {
    pub fn has_flag(&self, f: u32) -> bool {
        self.flags & f != 0
    }
    pub fn is_read_only(&self) -> bool {
        self.has_flag(flags::READ_ONLY)
    }
    pub fn is_required(&self) -> bool {
        self.has_flag(flags::REQUIRED)
    }
    pub fn is_multiline(&self) -> bool {
        self.kind == FieldKind::Text && self.has_flag(flags::MULTILINE)
    }
    pub fn is_password(&self) -> bool {
        self.kind == FieldKind::Text && self.has_flag(flags::PASSWORD)
    }
    /// Comb fields need `/MaxLen` and exclude multiline, password and file-select.
    pub fn is_comb(&self) -> bool {
        self.kind == FieldKind::Text
            && self.has_flag(flags::COMB)
            && self.max_len.is_some()
            && !self.has_flag(flags::MULTILINE | flags::PASSWORD | flags::FILE_SELECT)
    }
    pub fn is_rich_text(&self) -> bool {
        self.kind == FieldKind::Text && self.has_flag(flags::RICH_TEXT)
    }
    pub fn is_multi_select(&self) -> bool {
        self.kind == FieldKind::ListBox && self.has_flag(flags::MULTI_SELECT)
    }
    pub fn is_editable_combo(&self) -> bool {
        self.kind == FieldKind::ComboBox && self.has_flag(flags::EDIT)
    }
    pub fn action(&self, trigger: &str) -> Option<&Action> {
        self.actions
            .iter()
            .find(|a| a.trigger == trigger)
            .map(|a| &a.action)
    }
    /// Checkbox on, or radio with a selected state. `None` for other kinds.
    pub fn is_checked(&self) -> Option<bool> {
        match self.kind {
            FieldKind::Checkbox | FieldKind::Radio => Some(match &self.value {
                FieldValue::Name(n) => n != "Off",
                _ => self.widgets.iter().any(|w| w.is_on),
            }),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct Form {
    /// Every node of the tree (non-terminal groups included), in depth-first order.
    pub fields: Vec<Field>,
    /// Indices of top-level fields (`/Fields`).
    pub roots: Vec<usize>,
    /// `/CO`: terminal fields in calculation order.
    pub calc_order: Vec<usize>,
    pub has_acroform: bool,
    pub need_appearances: bool,
    /// `/SigFlags`: 1 = signatures exist, 2 = append only.
    pub sig_flags: u32,
    pub default_appearance: Option<String>,
    pub quadding: u8,
    /// `/DR`.
    pub default_resources: Option<Object>,
    pub xfa: XfaKind,
    /// Catalog `/NeedsRendering`.
    pub needs_rendering: bool,
    by_name: HashMap<String, usize>,
    by_id: HashMap<ObjId, usize>,
    by_widget: HashMap<ObjId, (usize, usize)>,
}

impl Form {
    pub fn terminal_fields(&self) -> impl Iterator<Item = &Field> {
        self.fields.iter().filter(|f| f.is_terminal)
    }
    pub fn find(&self, qualified_name: &str) -> Option<&Field> {
        self.by_name.get(qualified_name).map(|&i| &self.fields[i])
    }
    pub fn field_by_id(&self, id: ObjId) -> Option<&Field> {
        self.by_id.get(&id).map(|&i| &self.fields[i])
    }
    pub fn index_of(&self, id: ObjId) -> Option<usize> {
        self.by_id.get(&id).copied()
    }
    /// The field and widget index owning a widget annotation.
    pub fn owner_of_widget(&self, widget: ObjId) -> Option<(&Field, usize)> {
        self.by_widget
            .get(&widget)
            .map(|&(f, w)| (&self.fields[f], w))
    }
    pub fn widget_count(&self) -> usize {
        self.fields.iter().map(|f| f.widgets.len()).sum()
    }
    /// Widgets on one page, with their owning field index.
    pub fn widgets_on_page(&self, page: usize) -> Vec<(usize, &Widget)> {
        let mut out = Vec::new();
        for (i, f) in self.fields.iter().enumerate() {
            for w in &f.widgets {
                if w.page == Some(page) {
                    out.push((i, w));
                }
            }
        }
        out
    }
    pub fn signature_fields(&self) -> impl Iterator<Item = &Field> {
        self.fields
            .iter()
            .filter(|f| f.kind == FieldKind::Signature)
    }
}

/// Page of every annotation referenced from a page's `/Annots`, plus their document order.
#[derive(Debug, Default)]
pub struct AnnotPageMap {
    pub pages: HashMap<ObjId, usize>,
    pub order: Vec<ObjId>,
}

struct Builder<'a> {
    m: &'a Model,
    fields: Vec<Field>,
    seen: HashSet<ObjId>,
    annot_pages: Option<Rc<AnnotPageMap>>,
    acro_da: Option<String>,
    acro_q: u8,
    truncated: bool,
}

impl Model {
    pub fn form(&self) -> Result<Rc<Form>> {
        self.cached(Key::Form, |m| Ok(m.build_form()))
    }

    pub(crate) fn annot_page_map(&self) -> Result<Rc<AnnotPageMap>> {
        self.cached(Key::AnnotPageMap, |m| {
            let mut map = AnnotPageMap::default();
            let n = m.page_count()?;
            for i in 0..n {
                let page = m.page_object(i)?;
                for a in m.items_of(&page, "Annots") {
                    if let Some(id) = a.id()
                        && map.pages.insert(id, i).is_none()
                    {
                        map.order.push(id);
                    }
                }
            }
            Ok(map)
        })
    }

    fn build_form(&self) -> Form {
        let mut form = Form {
            fields: Vec::new(),
            roots: Vec::new(),
            calc_order: Vec::new(),
            has_acroform: false,
            need_appearances: false,
            sig_flags: 0,
            default_appearance: None,
            quadding: 0,
            default_resources: None,
            xfa: XfaKind::None,
            needs_rendering: false,
            by_name: HashMap::new(),
            by_id: HashMap::new(),
            by_widget: HashMap::new(),
        };
        let Some(root) = self.root() else {
            return form;
        };
        form.needs_rendering = self.bool_of(&root, "NeedsRendering").unwrap_or(false);
        let Some(acro) = self.get(&root, "AcroForm") else {
            return form;
        };
        if !self.is_dict(&acro) {
            return form;
        }
        form.has_acroform = true;
        form.need_appearances = self.bool_of(&acro, "NeedAppearances").unwrap_or(false);
        form.sig_flags = self.int_of(&acro, "SigFlags").unwrap_or(0).max(0) as u32;
        form.default_appearance = self.get(&acro, "DA").and_then(|d| self.text(&d));
        form.quadding = self.int_of(&acro, "Q").unwrap_or(0).clamp(0, 2) as u8;
        form.default_resources = self.get(&acro, "DR");
        form.xfa = self.xfa_kind(&acro, form.needs_rendering);

        let mut b = Builder {
            m: self,
            fields: Vec::new(),
            seen: HashSet::new(),
            annot_pages: None,
            acro_da: form.default_appearance.clone(),
            acro_q: form.quadding,
            truncated: false,
        };
        for f in self.items_of(&acro, "Fields") {
            if let Some(i) = b.node(&f, None, "", 0) {
                form.roots.push(i);
            }
        }
        // Widgets that sit on pages but not in the field tree are adopted as fields, as qpdf and
        // PDFium do, so that every visible control is reachable.
        if let Ok(map) = self.annot_page_map() {
            let mut known: HashSet<ObjId> = b.fields.iter().map(|f| f.id).collect();
            for f in &b.fields {
                known.extend(f.widgets.iter().map(|w| w.id));
            }
            for id in &map.order {
                if known.contains(id) {
                    continue;
                }
                let Ok(obj) = self.document().object(*id) else {
                    continue;
                };
                if self.name_of(&obj, "Subtype").as_deref() != Some("Widget") {
                    continue;
                }
                let parent_name = self.parent_qualified_name(&obj);
                if let Some(i) = b.node(&obj, None, &parent_name, 0) {
                    b.fields[i].orphan = true;
                    form.roots.push(i);
                }
            }
        }
        if b.truncated {
            self.note("form field walk truncated");
        }
        form.fields = b.fields;
        for (i, f) in form.fields.iter().enumerate() {
            form.by_id.insert(f.id, i);
            form.by_name.entry(f.qualified_name.clone()).or_insert(i);
            for (w, widget) in f.widgets.iter().enumerate() {
                form.by_widget.insert(widget.id, (i, w));
            }
        }
        for c in self.items_of(&acro, "CO") {
            if let Some(&i) = c.id().and_then(|id| form.by_id.get(&id)) {
                form.calc_order.push(i);
            }
        }
        form
    }

    /// Dot-joined partial names of an object's ancestors.
    fn parent_qualified_name(&self, obj: &Object) -> String {
        let mut parts = Vec::new();
        let mut cur = self.get(obj, "Parent");
        let mut seen = HashSet::new();
        while let Some(p) = cur {
            if !self.is_dict(&p)
                || parts.len() > MAX_DEPTH
                || p.id().is_some_and(|i| !seen.insert(i))
            {
                break;
            }
            if let Some(t) = self.text_of(&p, "T") {
                parts.push(t);
            }
            cur = self.get(&p, "Parent");
        }
        parts.reverse();
        parts.retain(|p| !p.is_empty());
        parts.join(".")
    }

    fn xfa_kind(&self, acro: &Object, needs_rendering: bool) -> XfaKind {
        let Some(xfa) = self.get(acro, "XFA") else {
            return XfaKind::None;
        };
        // The packet carrying `<template ... baseProfile="interactiveForms">` decides.
        let template = match self.kind(&xfa) {
            ObjectKind::Array => {
                let items = self.items(&xfa);
                items
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .find(|p| self.bytes(&p[0]).as_deref() == Some(b"template"))
                    .map(|p| p[1].clone())
            }
            ObjectKind::Stream => Some(xfa.clone()),
            _ => None,
        };
        let Some(t) = template else {
            return if needs_rendering {
                XfaKind::Dynamic
            } else {
                XfaKind::Static
            };
        };
        let Ok(data) = t.stream_decoded(DecodeLevel::Generalized) else {
            return XfaKind::Dynamic;
        };
        let bytes = data.as_slice();
        let Some(pos) = find(bytes, b"<template") else {
            return XfaKind::Dynamic;
        };
        let head = &bytes[pos..bytes.len().min(pos + 1024)];
        let head = &head[..find(head, b">").unwrap_or(head.len())];
        if find(head, b"interactiveForms").is_some() {
            XfaKind::Static
        } else {
            XfaKind::Dynamic
        }
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

impl Builder<'_> {
    /// Add `node` (and its descendants) to the arena. Returns its index.
    fn node(
        &mut self,
        node: &Object,
        parent: Option<usize>,
        parent_name: &str,
        depth: usize,
    ) -> Option<usize> {
        let m = self.m;
        if !m.is_dict(node) || depth > MAX_DEPTH {
            if depth > MAX_DEPTH {
                self.truncated = true;
            }
            return None;
        }
        if self.fields.len() >= MAX_NODES {
            self.truncated = true;
            return None;
        }
        if let Some(id) = node.id()
            && !self.seen.insert(id)
        {
            m.note(format!("form field cycle or duplicate at {id}"));
            return None;
        }
        let name = m.text_of(node, "T").unwrap_or_default();
        let qualified = match (parent_name.is_empty(), name.is_empty()) {
            (_, true) => parent_name.to_string(),
            (true, false) => name.clone(),
            (false, false) => format!("{parent_name}.{name}"),
        };

        // Kids that have their own /T are child fields; the rest are widgets of this field.
        let kids = m.items_of(node, "Kids");
        let (field_kids, widget_kids): (Vec<Object>, Vec<Object>) = kids
            .into_iter()
            .filter(|k| m.is_dict(k))
            .partition(|k| m.get(k, "T").is_some());
        let is_terminal = field_kids.is_empty();

        let idx = self.fields.len();
        let field = self.make_field(node, parent, &name, &qualified, is_terminal);
        self.fields.push(field);

        let mut children = Vec::new();
        for k in &field_kids {
            if let Some(c) = self.node(k, Some(idx), &qualified, depth + 1) {
                children.push(c);
            }
        }
        self.fields[idx].children = children;

        if is_terminal {
            let widget_objs: Vec<Object> = if widget_kids.is_empty() {
                // Merged field/widget dictionary (unless it is a widgetless terminal).
                if m.name_of(node, "Subtype").as_deref() == Some("Widget")
                    || m.get(node, "Rect").is_some()
                {
                    vec![node.clone()]
                } else {
                    Vec::new()
                }
            } else {
                widget_kids
            };
            let mut widgets = Vec::new();
            let value = self.fields[idx].value.clone();
            for w in &widget_objs {
                if let Some(widget) = self.make_widget(w, &value) {
                    widgets.push(widget);
                }
            }
            let f = &mut self.fields[idx];
            if matches!(f.kind, FieldKind::Checkbox | FieldKind::Radio) {
                f.export_values = widgets.iter().filter_map(|w| w.on_state.clone()).collect();
            }
            f.widgets = widgets;
        }
        Some(idx)
    }

    fn make_field(
        &self,
        node: &Object,
        parent: Option<usize>,
        name: &str,
        qualified: &str,
        is_terminal: bool,
    ) -> Field {
        let m = self.m;
        let flags = m.inherited(node, "Ff").and_then(|v| m.int(&v)).unwrap_or(0) as u32;
        let ft = m.inherited(node, "FT").and_then(|v| m.name(&v));
        let kind = match ft.as_deref() {
            Some("Tx") => FieldKind::Text,
            Some("Btn") => {
                if flags & flags::PUSH_BUTTON != 0 {
                    FieldKind::PushButton
                } else if flags & flags::RADIO != 0 {
                    FieldKind::Radio
                } else {
                    FieldKind::Checkbox
                }
            }
            Some("Ch") => {
                if flags & flags::COMBO != 0 {
                    FieldKind::ComboBox
                } else {
                    FieldKind::ListBox
                }
            }
            Some("Sig") => FieldKind::Signature,
            _ => FieldKind::Unknown,
        };
        let value = |key: &str| -> FieldValue {
            match m.inherited(node, key) {
                Some(v) => self.field_value(&v),
                None => FieldValue::None,
            }
        };
        let options = m
            .inherited(node, "Opt")
            .map(|o| {
                m.items(&o)
                    .iter()
                    .filter_map(|e| {
                        if let Some(s) = m.text(e) {
                            return Some(ChoiceOption {
                                export: s.clone(),
                                display: s,
                            });
                        }
                        let pair = m.items(e);
                        (pair.len() >= 2).then(|| ChoiceOption {
                            export: m.text(&pair[0]).unwrap_or_default(),
                            display: m.text(&pair[1]).unwrap_or_default(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut actions = Vec::new();
        if let Some(aa) = m.get(node, "AA") {
            for trigger in [
                "K", "F", "V", "C", "E", "X", "D", "U", "Fo", "Bl", "PO", "PC", "PV", "PI",
            ] {
                if let Some(a) = m.get(&aa, trigger).and_then(|a| m.parse_action(&a)) {
                    actions.push(FieldAction {
                        trigger: trigger.to_string(),
                        action: a,
                    });
                }
            }
        }
        Field {
            id: node.id().unwrap_or(ObjId::new(0, 0)),
            name: name.to_string(),
            qualified_name: qualified.to_string(),
            kind,
            flags,
            parent,
            children: Vec::new(),
            is_terminal,
            value: value("V"),
            default_value: value("DV"),
            default_appearance: m
                .inherited(node, "DA")
                .and_then(|d| m.text(&d))
                .or_else(|| self.acro_da.clone()),
            quadding: m
                .inherited(node, "Q")
                .and_then(|q| m.int(&q))
                .map_or(self.acro_q, |q| q.clamp(0, 2) as u8),
            max_len: m
                .inherited(node, "MaxLen")
                .and_then(|v| m.int(&v))
                .filter(|&n| n > 0)
                .map(|n| n.min(i64::from(u32::MAX)) as u32),
            options,
            top_index: m
                .inherited(node, "TI")
                .and_then(|v| m.int(&v))
                .filter(|&n| n >= 0)
                .map(|n| n as u32),
            selected_indices: m
                .inherited(node, "I")
                .map(|a| {
                    m.items(&a)
                        .iter()
                        .filter_map(|e| m.int(e))
                        .filter(|&i| i >= 0)
                        .map(|i| i as u32)
                        .collect()
                })
                .unwrap_or_default(),
            tooltip: m.text_of(node, "TU"),
            mapping_name: m.text_of(node, "TM"),
            export_values: Vec::new(),
            actions,
            widgets: Vec::new(),
            orphan: false,
        }
    }

    fn field_value(&self, v: &Object) -> FieldValue {
        let m = self.m;
        match m.kind(v) {
            ObjectKind::String => FieldValue::Text(m.text(v).unwrap_or_default()),
            ObjectKind::Stream => FieldValue::Text(m.text_or_stream(v).unwrap_or_default()),
            ObjectKind::Name => FieldValue::Name(m.name(v).unwrap_or_default()),
            ObjectKind::Array => FieldValue::List(
                m.items(v)
                    .iter()
                    .filter_map(|e| m.text(e).or_else(|| m.name(e)))
                    .collect(),
            ),
            _ => FieldValue::None,
        }
    }

    fn make_widget(&mut self, w: &Object, field_value: &FieldValue) -> Option<Widget> {
        let m = self.m;
        let id = w.id()?;
        let rect = m.rect_of(w, "Rect").unwrap_or_default();
        let ap_n = m.get(w, "AP").and_then(|ap| m.get(&ap, "N"));
        let ap_states: Vec<String> = match &ap_n {
            Some(n) if m.kind(n) == ObjectKind::Dictionary => m.dict_keys(n),
            _ => Vec::new(),
        };
        let on_state = ap_states
            .iter()
            .find(|s| s.as_str() != "Off")
            .cloned()
            .or_else(|| {
                // Some files keep only the pressed appearance under /D.
                let d = m.get(w, "AP").and_then(|ap| m.get(&ap, "D"))?;
                m.dict_keys(&d).into_iter().find(|s| s != "Off")
            });
        let appearance_state = m.name_of(w, "AS");
        let is_on = match &appearance_state {
            Some(s) => s != "Off",
            None => match (field_value, &on_state) {
                (FieldValue::Name(v), Some(on)) => v == on,
                _ => false,
            },
        };
        let mk = m.get(w, "MK");
        let color = |key: &str| -> Option<Vec<f64>> {
            let c = m.numbers(&mk.as_ref().and_then(|d| m.get(d, key))?);
            (!c.is_empty()).then_some(c)
        };
        let page = {
            let from_p = m
                .get(w, "P")
                .and_then(|p| p.id())
                .and_then(|id| m.page_list().ok().and_then(|l| l.index_of(id)));
            if self.annot_pages.is_none() {
                self.annot_pages = m.annot_page_map().ok();
            }
            self.annot_pages
                .as_ref()
                .and_then(|map| map.pages.get(&id).copied())
                .or(from_p)
        };
        Some(Widget {
            id,
            rect,
            page,
            flags: m.int_of(w, "F").unwrap_or(0) as u32,
            appearance_state,
            on_state,
            ap_states,
            has_appearance: ap_n.is_some(),
            is_on,
            border_color: color("BC"),
            background: color("BG"),
            caption: mk.as_ref().and_then(|d| m.text_of(d, "CA")),
            rotation: mk
                .as_ref()
                .and_then(|d| m.num_of(d, "R"))
                .map_or(0, normalize_rotation),
            border: m.border_style(w),
            highlight: m.name_of(w, "H").and_then(|h| h.chars().next()),
            action: m.get(w, "A").and_then(|a| m.parse_action(&a)),
        })
    }
}
