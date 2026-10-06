//! Form fields across page copies: cloning widget annotations with their field ancestors,
//! registering the new top-level fields in `/AcroForm` (renaming on collision), and removing
//! fields from deleted pages.

use std::collections::{HashMap, HashSet};

use papyrine_cos::{Document, ObjId, Object, ObjectKind};

use crate::context::EditContext;
use crate::error::Result;
use crate::image::deep_copy_direct;
use crate::text::{decode_text_string, encode_text_string};

const MAX_DEPTH: usize = 64;

pub(crate) fn is_widget(a: &Object) -> Result<bool> {
    Ok(a.kind()? == ObjectKind::Dictionary
        && a.dict_get("Subtype")?.name().is_ok_and(|n| n == b"Widget"))
}

/// Copies dictionaries (without their `/Parent` and `/Kids`) from a source document into a
/// destination, either the same document or a foreign one.
pub(crate) struct Cloner<'a> {
    pub dest: &'a Document,
    pub src: &'a Document,
    pub foreign: bool,
}

impl Cloner<'_> {
    /// A new indirect object in `dest` holding a copy of the dictionary `obj` minus `drop`.
    /// Indirect values (appearance streams, fonts) are shared (same document) or copied once
    /// per source object (foreign).
    pub fn clone_dict(&self, obj: &Object, drop: &[&str]) -> Result<Object> {
        let tmp = deep_copy_direct(self.src, obj)?;
        for k in drop {
            tmp.dict_remove(k)?;
        }
        let tmp = self.src.make_indirect(&tmp)?;
        if self.foreign {
            Ok(self.dest.copy_foreign(&tmp)?)
        } else {
            Ok(tmp)
        }
    }

    /// A copy of any value (direct or indirect) usable in `dest`.
    pub fn copy_value(&self, v: &Object) -> Result<Object> {
        if !self.foreign {
            return Ok(v.clone());
        }
        let holder = self.src.make_indirect(&self.src.new_array())?;
        holder.array_push(v)?;
        let copy = self.dest.copy_foreign(&holder)?;
        Ok(copy.array_get(0)?)
    }
}

/// The widget clones of one import, with the shared field ancestors built once.
pub(crate) struct FieldCloner<'a> {
    pub cloner: Cloner<'a>,
    ancestors: HashMap<ObjId, Object>,
    pub tops: Vec<Object>,
}

impl<'a> FieldCloner<'a> {
    pub fn new(cloner: Cloner<'a>) -> Self {
        FieldCloner {
            cloner,
            ancestors: HashMap::new(),
            tops: Vec::new(),
        }
    }

    /// Clone `w` (a widget of the source) for the page copy `page` and hook it into a field
    /// tree rebuilt from its ancestors. Siblings on other pages are not copied.
    pub fn widget(&mut self, w: &Object, page: &Object) -> Result<Object> {
        let nw = self.cloner.clone_dict(w, &["Parent", "P"])?;
        nw.dict_set("P", page)?;
        let (mut child_src, mut child_new) = (w.clone(), nw.clone());
        for _ in 0..MAX_DEPTH {
            let parent = child_src.dict_get("Parent")?;
            if parent.kind()? != ObjectKind::Dictionary {
                self.tops.push(child_new);
                break;
            }
            let Some(pid) = parent.id() else {
                self.tops.push(child_new);
                break;
            };
            if let Some(known) = self.ancestors.get(&pid) {
                known.dict_get("Kids")?.array_push(&child_new)?;
                child_new.dict_set("Parent", known)?;
                break;
            }
            let np = self.cloner.clone_dict(&parent, &["Parent", "Kids"])?;
            let kids = self.cloner.dest.new_array();
            kids.array_push(&child_new)?;
            np.dict_set("Kids", &kids)?;
            child_new.dict_set("Parent", &np)?;
            self.ancestors.insert(pid, np.clone());
            child_src = parent;
            child_new = np;
        }
        Ok(nw)
    }
}

fn touch_dr(cx: &mut EditContext<'_>, af: &Object) -> Result<()> {
    let dr = af.dict_get("DR")?;
    if dr.kind()? != ObjectKind::Dictionary {
        return Ok(());
    }
    if dr.is_indirect() {
        cx.touch(&dr)?;
    }
    for k in dr.dict_keys()? {
        let v = dr.dict_get(&k)?;
        if v.is_indirect() && v.kind()? == ObjectKind::Dictionary {
            cx.touch(&v)?;
        }
    }
    Ok(())
}

/// Record the catalog and every indirect piece of `/AcroForm` that registering fields can
/// change.
pub(crate) fn touch_acroform(cx: &mut EditContext<'_>) -> Result<()> {
    let root = cx.doc().root()?;
    cx.touch(&root)?;
    let af = root.dict_get("AcroForm")?;
    if af.kind()? != ObjectKind::Dictionary {
        return Ok(());
    }
    if af.is_indirect() {
        cx.touch(&af)?;
    }
    let fields = af.dict_get("Fields")?;
    if fields.is_indirect() && fields.kind()? == ObjectKind::Array {
        cx.touch(&fields)?;
    }
    touch_dr(cx, &af)
}

/// The destination's `/AcroForm` (created when missing) and its `/Fields` array. The caller has
/// already run [`touch_acroform`].
fn ensure_acroform(doc: &Document) -> Result<(Object, Object)> {
    let root = doc.root()?;
    let mut af = root.dict_get("AcroForm")?;
    if af.kind()? != ObjectKind::Dictionary {
        af = doc.make_indirect(&doc.new_dict())?;
        root.dict_set("AcroForm", &af)?;
    }
    let mut fields = af.dict_get("Fields")?;
    if fields.kind()? != ObjectKind::Array {
        fields = doc.new_array();
        af.dict_set("Fields", &fields)?;
        fields = af.dict_get("Fields")?;
    }
    Ok((af, fields))
}

fn text_of(o: &Object) -> Result<Option<String>> {
    Ok((o.kind()? == ObjectKind::String)
        .then(|| decode_text_string(&o.string().unwrap_or_default())))
}

/// Remove signature values from the cloned tree: a copied signature no longer covers the
/// document it was applied to.
fn strip_signatures(node: &Object, inherited_ft: Option<Vec<u8>>, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH || node.kind()? != ObjectKind::Dictionary {
        return Ok(());
    }
    let ft = node.dict_get("FT")?.name().ok().or(inherited_ft);
    if ft.as_deref() == Some(b"Sig") && node.dict_has("V")? {
        node.dict_remove("V")?;
    }
    let kids = node.dict_get("Kids")?;
    if kids.kind()? == ObjectKind::Array {
        for k in kids.array_items()? {
            strip_signatures(&k, ft.clone(), depth + 1)?;
        }
    }
    Ok(())
}

/// What registering did, for reports.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FieldReport {
    pub fields: usize,
    /// `(old, new)` top-level names changed to avoid collisions.
    pub renamed: Vec<(String, String)>,
}

/// Add `tops` (new top-level fields) to the destination `/AcroForm`: rename on collision with
/// an existing top-level name, inherit the source's `/DA`, `/Q` and default resources, and drop
/// signature values.
pub(crate) fn register_fields(
    cx: &mut EditContext<'_>,
    tops: &[Object],
    src: &Document,
    cloner: &Cloner<'_>,
) -> Result<FieldReport> {
    let mut report = FieldReport::default();
    if tops.is_empty() {
        return Ok(report);
    }
    touch_acroform(cx)?;
    let doc = cx.doc();
    let (af, fields) = ensure_acroform(doc)?;
    let mut taken: HashSet<String> = HashSet::new();
    for f in fields.array_items()? {
        if f.kind()? == ObjectKind::Dictionary
            && let Some(t) = text_of(&f.dict_get("T")?)?
        {
            taken.insert(t);
        }
    }
    let src_af = src.root()?.dict_get("AcroForm")?;
    let has_src_af = src_af.kind()? == ObjectKind::Dictionary;
    let src_da = if has_src_af {
        src_af.dict_get("DA")?
    } else {
        src.new_null()
    };
    let dest_da = af.dict_get("DA")?;
    let src_q = if has_src_af {
        src_af.dict_get("Q")?
    } else {
        src.new_null()
    };
    let dest_q = af.dict_get("Q")?;
    let da_differs = src_da.kind()? == ObjectKind::String
        && (dest_da.kind()? != ObjectKind::String || dest_da.string()? != src_da.string()?);
    let q_differs = src_q.kind()? == ObjectKind::Integer
        && (dest_q.kind()? != ObjectKind::Integer || dest_q.as_int()? != src_q.as_int()?);
    if src_da.kind()? == ObjectKind::String && dest_da.kind()? != ObjectKind::String {
        af.dict_set("DA", &doc.new_string(src_da.string()?)?)?;
    }
    if src_q.kind()? == ObjectKind::Integer && dest_q.kind()? != ObjectKind::Integer {
        af.dict_set("Q", &doc.new_int(src_q.as_int()?))?;
    }
    for top in tops {
        // The destination default differs: make the copied tree keep the source's.
        if da_differs && !top.dict_has("DA")? {
            top.dict_set("DA", &doc.new_string(src_da.string()?)?)?;
        }
        if q_differs && !top.dict_has("Q")? {
            top.dict_set("Q", &doc.new_int(src_q.as_int()?))?;
        }
        strip_signatures(top, None, 0)?;
        if let Some(name) = text_of(&top.dict_get("T")?)?
            && !name.is_empty()
        {
            let fresh = if taken.contains(&name) {
                (2..)
                    .map(|k| format!("{name}_{k}"))
                    .find(|n| !taken.contains(n))
                    .expect("unbounded search")
            } else {
                name.clone()
            };
            if fresh != name {
                top.dict_set("T", &doc.new_string(encode_text_string(&fresh))?)?;
                report.renamed.push((name, fresh.clone()));
            }
            taken.insert(fresh);
        }
        fields.array_push(top)?;
        report.fields += 1;
    }
    if has_src_af {
        merge_default_resources(cx, &af, &src_af, cloner)?;
        if src_af
            .dict_get("NeedAppearances")?
            .as_bool()
            .unwrap_or(false)
        {
            af.dict_set("NeedAppearances", &doc.new_bool(true))?;
        }
    }
    Ok(report)
}

/// Add the source's `/DR` resources that the destination lacks. Existing names keep the
/// destination's definition.
fn merge_default_resources(
    cx: &mut EditContext<'_>,
    af: &Object,
    src_af: &Object,
    cloner: &Cloner<'_>,
) -> Result<()> {
    let doc = cx.doc();
    let src_dr = src_af.dict_get("DR")?;
    if src_dr.kind()? != ObjectKind::Dictionary {
        return Ok(());
    }
    let mut dr = af.dict_get("DR")?;
    if dr.kind()? != ObjectKind::Dictionary {
        dr = doc.new_dict();
        af.dict_set("DR", &dr)?;
        dr = af.dict_get("DR")?;
    }
    for cat in src_dr.dict_keys()? {
        let src_cat = src_dr.dict_get(&cat)?;
        if src_cat.kind()? != ObjectKind::Dictionary {
            continue;
        }
        let mut dest_cat = dr.dict_get(&cat)?;
        if dest_cat.kind()? != ObjectKind::Dictionary {
            dest_cat = doc.new_dict();
            dr.dict_set(&cat, &dest_cat)?;
            dest_cat = dr.dict_get(&cat)?;
        }
        for name in src_cat.dict_keys()? {
            if !dest_cat.dict_has(&name)? {
                dest_cat.dict_set(&name, &cloner.copy_value(&src_cat.dict_get(&name)?)?)?;
            }
        }
    }
    Ok(())
}

/// Remove every widget in `removed_pages` from the form: widgets leave their field's `/Kids`,
/// and fields left without widgets leave their parent (or `/AcroForm /Fields`). Without this a
/// deleted page stays reachable through its widgets' `/P`.
pub(crate) fn drop_widgets_of_pages(
    cx: &mut EditContext<'_>,
    removed: &HashSet<ObjId>,
) -> Result<()> {
    let doc = cx.doc();
    let root = doc.root()?;
    let af = root.dict_get("AcroForm")?;
    if af.kind()? != ObjectKind::Dictionary {
        return Ok(());
    }
    let fields = af.dict_get("Fields")?;
    if fields.kind()? != ObjectKind::Array {
        return Ok(());
    }
    // Find the doomed widgets first so nothing is touched when none exist.
    let mut doomed = false;
    let mut stack: Vec<(Object, usize)> =
        fields.array_items()?.into_iter().map(|f| (f, 0)).collect();
    let mut seen = HashSet::new();
    while let Some((n, d)) = stack.pop() {
        if d > MAX_DEPTH || n.kind()? != ObjectKind::Dictionary {
            continue;
        }
        if let Some(id) = n.id()
            && !seen.insert(id)
        {
            continue;
        }
        if is_widget(&n)? && widget_on(&n, removed)? {
            doomed = true;
            break;
        }
        let kids = n.dict_get("Kids")?;
        if kids.kind()? == ObjectKind::Array {
            stack.extend(kids.array_items()?.into_iter().map(|k| (k, d + 1)));
        }
    }
    if !doomed {
        return Ok(());
    }
    touch_acroform(cx)?;
    let kept = prune_kids(cx, &fields, removed, 0)?;
    debug_assert!(kept <= fields.array_len()?);
    Ok(())
}

fn widget_on(w: &Object, removed: &HashSet<ObjId>) -> Result<bool> {
    let p = w.dict_get("P")?;
    Ok(p.kind()? == ObjectKind::Dictionary && p.id().is_some_and(|i| removed.contains(&i)))
}

/// Filter a `/Kids` or `/Fields` array in place; returns how many entries remain.
fn prune_kids(
    cx: &mut EditContext<'_>,
    arr: &Object,
    removed: &HashSet<ObjId>,
    depth: usize,
) -> Result<usize> {
    let items = arr.array_items()?;
    let mut keep_flags = Vec::with_capacity(items.len());
    for it in &items {
        keep_flags.push(keep_node(cx, it, removed, depth + 1)?);
    }
    for i in (0..items.len()).rev() {
        if !keep_flags[i] {
            arr.array_remove(i)?;
        }
    }
    Ok(keep_flags.iter().filter(|k| **k).count())
}

fn keep_node(
    cx: &mut EditContext<'_>,
    node: &Object,
    removed: &HashSet<ObjId>,
    depth: usize,
) -> Result<bool> {
    if depth > MAX_DEPTH || node.kind()? != ObjectKind::Dictionary {
        return Ok(true);
    }
    if is_widget(node)? && !node.dict_has("Kids")? {
        return Ok(!widget_on(node, removed)?);
    }
    let kids = node.dict_get("Kids")?;
    if kids.kind()? != ObjectKind::Array {
        return Ok(true);
    }
    // Only touch nodes whose Kids really change.
    let before = kids.array_len()?;
    let mut would_change = false;
    for k in kids.array_items()? {
        if !subtree_clean(&k, removed, depth + 1)? {
            would_change = true;
            break;
        }
    }
    if !would_change {
        return Ok(true);
    }
    if node.is_indirect() {
        cx.touch(node)?;
    }
    if kids.is_indirect() {
        cx.touch(&kids)?;
    }
    let left = prune_kids(cx, &kids, removed, depth)?;
    debug_assert!(left <= before);
    Ok(left > 0)
}

/// No widget below `n` sits on a removed page.
fn subtree_clean(n: &Object, removed: &HashSet<ObjId>, depth: usize) -> Result<bool> {
    if depth > MAX_DEPTH || n.kind()? != ObjectKind::Dictionary {
        return Ok(true);
    }
    if is_widget(n)? && widget_on(n, removed)? {
        return Ok(false);
    }
    let kids = n.dict_get("Kids")?;
    if kids.kind()? == ObjectKind::Array {
        for k in kids.array_items()? {
            if !subtree_clean(&k, removed, depth + 1)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
