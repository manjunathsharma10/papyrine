//! Tab order and field navigation (ISO 32000-2 section 12.5.2, `/Tabs`).
//!
//! Per page the order is `R` (rows: top to bottom, left to right), `C` (columns: left to right,
//! top to bottom), `S` (structure order, from the structure tree) or, when `/Tabs` is absent,
//! the order of the page's `/Annots` array. Radio groups and other multi-widget fields are one
//! tab stop each on a page (the selected button, else the first), as in Acrobat.

use std::collections::{HashMap, HashSet};

use papyrine_cos::{Document, ObjId, Object, ObjectKind};

use crate::cosx;
use crate::error::Result;
use crate::form::{Field, FormTree, Kind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabsMode {
    Rows,
    Columns,
    Structure,
    /// `/Tabs` absent: annotation order.
    Annotations,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TabStop {
    pub field: ObjId,
    pub name: String,
    pub widget: usize,
    pub page: usize,
    pub rect: [f64; 4],
}

fn page_widgets(page: &Object) -> Vec<Object> {
    cosx::get(page, "Annots")
        .map(|a| cosx::items(&a))
        .unwrap_or_default()
}

pub fn tabs_mode(page: &Object) -> TabsMode {
    match cosx::get_name(page, "Tabs").as_deref() {
        Some("R") => TabsMode::Rows,
        Some("C") => TabsMode::Columns,
        Some("S") => TabsMode::Structure,
        _ => TabsMode::Annotations,
    }
}

/// Widget object ids in structure-tree order (`/OBJR` references), document wide.
fn structure_order(doc: &Document) -> Vec<ObjId> {
    let mut out = Vec::new();
    let Ok(root) = doc.root() else { return out };
    let Some(st) = cosx::get(&root, "StructTreeRoot") else {
        return out;
    };
    let mut seen = HashSet::new();
    fn walk(node: &Object, out: &mut Vec<ObjId>, seen: &mut HashSet<ObjId>, depth: usize) {
        if depth > 64 || out.len() > 1_000_000 {
            return;
        }
        if let Some(id) = node.id()
            && !seen.insert(id)
        {
            return;
        }
        if cosx::kind(node) == Some(ObjectKind::Array) {
            for k in cosx::items(node) {
                walk(&k, out, seen, depth + 1);
            }
            return;
        }
        if !cosx::is_dict(node) {
            return;
        }
        if cosx::get_name(node, "Type").as_deref() == Some("OBJR") {
            if let Some(id) = cosx::get(node, "Obj").and_then(|o| o.id()) {
                out.push(id);
            }
            return;
        }
        if let Some(k) = cosx::get(node, "K") {
            walk(&k, out, seen, depth + 1);
        }
    }
    if let Some(k) = cosx::get(&st, "K") {
        walk(&k, &mut out, &mut seen, 0);
    }
    out
}

/// Tab stops of one page in tab order.
pub fn tab_order(doc: &Document, page_index: usize) -> Result<Vec<TabStop>> {
    let form = FormTree::load(doc)?;
    let page = doc.page(page_index)?;
    Ok(tab_order_in(doc, &form, &page, page_index))
}

fn tab_order_in(doc: &Document, form: &FormTree, page: &Object, page_index: usize) -> Vec<TabStop> {
    // widget object id -> (field index, widget index)
    let mut by_widget: HashMap<ObjId, (usize, usize)> = HashMap::new();
    for (fi, f) in form.fields.iter().enumerate() {
        for (wi, w) in f.widgets.iter().enumerate() {
            if let Some(id) = w.id {
                by_widget.insert(id, (fi, wi));
            }
        }
    }
    let mut stops: Vec<(ObjId, usize, usize)> = Vec::new(); // widget id, field idx, widget idx
    for a in page_widgets(page) {
        if let Some(id) = a.id()
            && let Some(&(fi, wi)) = by_widget.get(&id)
            && !form.fields[fi].widgets[wi].obj.is_null().unwrap_or(true)
        {
            stops.push((id, fi, wi));
        }
    }
    let rect_of = |s: &(ObjId, usize, usize)| form.fields[s.1].widgets[s.2].rect;
    match tabs_mode(page) {
        TabsMode::Annotations => {}
        TabsMode::Rows => {
            stops.sort_by(|a, b| {
                let (ra, rb) = (rect_of(a), rect_of(b));
                // Same row when vertical centres are within half the smaller height.
                let (ca, cb) = ((ra[1] + ra[3]) / 2.0, (rb[1] + rb[3]) / 2.0);
                let tol = ((ra[3] - ra[1]).min(rb[3] - rb[1]) / 2.0).max(1.0);
                if (ca - cb).abs() <= tol {
                    ra[0].total_cmp(&rb[0])
                } else {
                    cb.total_cmp(&ca)
                }
            });
        }
        TabsMode::Columns => {
            stops.sort_by(|a, b| {
                let (ra, rb) = (rect_of(a), rect_of(b));
                let tol = ((ra[2] - ra[0]).min(rb[2] - rb[0]) / 2.0).max(1.0);
                if (ra[0] - rb[0]).abs() <= tol {
                    rb[3].total_cmp(&ra[3])
                } else {
                    ra[0].total_cmp(&rb[0])
                }
            });
        }
        TabsMode::Structure => {
            let order = structure_order(doc);
            let pos: HashMap<ObjId, usize> =
                order.iter().enumerate().map(|(i, id)| (*id, i)).collect();
            // Stable: widgets missing from the tree keep annotation order after the others.
            stops.sort_by_key(|s| pos.get(&s.0).copied().unwrap_or(usize::MAX));
        }
    }
    // One stop per multi-widget field.
    let mut done: HashSet<usize> = HashSet::new();
    let mut out = Vec::new();
    for s in stops {
        let f: &Field = &form.fields[s.1];
        if matches!(f.kind, Kind::Signature | Kind::Other) && f.widgets.is_empty() {
            continue;
        }
        if f.kind == Kind::Radio && f.widgets.len() > 1 {
            if !done.insert(s.1) {
                continue;
            }
            // The selected button represents the group; else the first on this page.
            let on = stops_selected(f);
            let wi = on.unwrap_or(s.2);
            out.push(stop(f, wi, page_index));
            continue;
        }
        out.push(stop(f, s.2, page_index));
    }
    out
}

fn stops_selected(f: &Field) -> Option<usize> {
    f.widgets
        .iter()
        .position(|w| w.appearance_state.as_deref().is_some_and(|s| s != "Off"))
}

fn stop(f: &Field, wi: usize, page: usize) -> TabStop {
    TabStop {
        field: f.id,
        name: f.name.clone(),
        widget: wi,
        page,
        rect: f.widgets[wi].rect,
    }
}

/// Tab stops of the whole document, page by page.
pub fn document_tab_order(doc: &Document) -> Result<Vec<TabStop>> {
    let form = FormTree::load(doc)?;
    let mut all = Vec::new();
    for i in 0..doc.page_count()? {
        let page = doc.page(i)?;
        all.extend(tab_order_in(doc, &form, &page, i));
    }
    Ok(all)
}

/// The stop after (or before) `from`, wrapping around the document. `from: None` gives the first
/// (or last) stop. Fields that are read-only or push buttons are skipped when `fillable_only`.
pub fn next_field(
    doc: &Document,
    from: Option<(ObjId, usize)>,
    forward: bool,
    fillable_only: bool,
) -> Result<Option<TabStop>> {
    let form = FormTree::load(doc)?;
    let mut order = Vec::new();
    for i in 0..doc.page_count()? {
        let page = doc.page(i)?;
        order.extend(tab_order_in(doc, &form, &page, i));
    }
    if fillable_only {
        order.retain(|s| {
            form.by_id(s.field).is_some_and(|f| {
                !f.is_read_only()
                    && !matches!(f.kind, Kind::PushButton | Kind::Signature | Kind::Other)
            })
        });
    }
    if order.is_empty() {
        return Ok(None);
    }
    let cur = from.and_then(|(id, w)| {
        order
            .iter()
            .position(|s| s.field == id && s.widget == w)
            .or_else(|| order.iter().position(|s| s.field == id))
    });
    let n = order.len();
    let i = match (cur, forward) {
        (None, true) => 0,
        (None, false) => n - 1,
        (Some(c), true) => (c + 1) % n,
        (Some(c), false) => (c + n - 1) % n,
    };
    Ok(Some(order[i].clone()))
}
