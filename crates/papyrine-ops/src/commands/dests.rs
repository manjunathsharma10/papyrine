//! Destinations: named-destination lookup (name tree and the legacy `/Dests` dictionary),
//! explicit destination arrays, and the `/Dest` / `/A /D` slots of annotations and bookmarks.

use std::collections::{BTreeMap, HashSet};

use papyrine_cos::{Document, ObjId, Object, ObjectKind};

use crate::context::EditContext;
use crate::error::Result;

/// All named destinations of a document: name bytes -> value (a destination array or a
/// dictionary with `/D`). Names from the `/Names /Dests` tree win over the legacy dictionary.
pub(crate) struct DestMap {
    pub entries: BTreeMap<Vec<u8>, Object>,
}

impl DestMap {
    pub fn collect(doc: &Document) -> Result<DestMap> {
        let mut entries = BTreeMap::new();
        let root = doc.root()?;
        let legacy = root.dict_get("Dests")?;
        if legacy.kind()? == ObjectKind::Dictionary {
            for k in legacy.dict_keys()? {
                entries.insert(k.clone(), legacy.dict_get(&k)?);
            }
        }
        if let Some(tree) = doc.catalog_name_tree("Dests", false)? {
            for k in doc.name_tree_keys(&tree)? {
                if let Some(v) = doc.name_tree_get(&tree, &k)? {
                    entries.insert(k, v);
                }
            }
        }
        Ok(DestMap { entries })
    }

    pub fn names(&self) -> HashSet<Vec<u8>> {
        self.entries.keys().cloned().collect()
    }

    pub fn array(&self, name: &[u8]) -> Result<Option<Object>> {
        match self.entries.get(name) {
            Some(v) => dest_array(v),
            None => Ok(None),
        }
    }
}

/// The destination array inside a name-tree value.
pub(crate) fn dest_array(v: &Object) -> Result<Option<Object>> {
    Ok(match v.kind()? {
        ObjectKind::Array => Some(v.clone()),
        ObjectKind::Dictionary => {
            let d = v.dict_get("D")?;
            (d.kind()? == ObjectKind::Array).then_some(d)
        }
        _ => None,
    })
}

/// The page object id a destination array points at (`None` for a page-number or null target).
pub(crate) fn dest_page(arr: &Object) -> Result<Option<ObjId>> {
    if arr.array_len()? == 0 {
        return Ok(None);
    }
    let p = arr.array_get(0)?;
    Ok(if p.kind()? == ObjectKind::Dictionary {
        p.id()
    } else {
        None
    })
}

/// Where a holder (bookmark or annotation) keeps its destination.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Slot {
    Dest,
    ActionD,
}

pub(crate) enum Target {
    Explicit(Object),
    Named(Vec<u8>),
}

/// The destination of a bookmark or annotation, with the slot that holds it.
pub(crate) fn holder_dest(holder: &Object) -> Result<Option<(Slot, Target)>> {
    let classify = |o: Object| -> Result<Option<Target>> {
        Ok(match o.kind()? {
            ObjectKind::Array => Some(Target::Explicit(o)),
            ObjectKind::String => Some(Target::Named(o.string()?)),
            ObjectKind::Name => Some(Target::Named(o.name()?)),
            _ => None,
        })
    };
    if let Some(t) = classify(holder.dict_get("Dest")?)? {
        return Ok(Some((Slot::Dest, t)));
    }
    let a = holder.dict_get("A")?;
    if a.kind()? == ObjectKind::Dictionary
        && a.dict_get("S")?.name().is_ok_and(|n| n == b"GoTo")
        && let Some(t) = classify(a.dict_get("D")?)?
    {
        return Ok(Some((Slot::ActionD, t)));
    }
    Ok(None)
}

/// Resolve a target to its destination array through `names`.
pub(crate) fn resolve(names: &DestMap, t: &Target) -> Result<Option<Object>> {
    match t {
        Target::Explicit(a) => Ok(Some(a.clone())),
        Target::Named(n) => names.array(n),
    }
}

pub(crate) fn set_dest(holder: &Object, slot: Slot, value: &Object) -> Result<()> {
    match slot {
        Slot::Dest => holder.dict_set("Dest", value)?,
        Slot::ActionD => holder.dict_get("A")?.dict_set("D", value)?,
    }
    Ok(())
}

/// Remove the destination (and the whole `GoTo` action) from a holder.
pub(crate) fn clear_dest(holder: &Object, slot: Slot) -> Result<()> {
    match slot {
        Slot::Dest => holder.dict_remove("Dest")?,
        Slot::ActionD => holder.dict_remove("A")?,
    }
    Ok(())
}

/// Copy of a destination array that points at `page` instead (other elements are fit
/// parameters: names, numbers and nulls, copied by value).
pub(crate) fn retarget(doc: &Document, arr: &Object, page: &Object) -> Result<Object> {
    let out = doc.new_array();
    out.array_push(page)?;
    for i in 1..arr.array_len()? {
        out.array_push(&copy_scalar(doc, &arr.array_get(i)?)?)?;
    }
    Ok(out)
}

/// A direct copy of a small, reference-free value (resolved when it is indirect).
pub(crate) fn copy_scalar(doc: &Document, v: &Object) -> Result<Object> {
    Ok(doc.parse_object(v.unparse_with(true)?)?)
}

/// A shallow copy of a destination array (first element, the page reference, kept as a
/// reference).
pub(crate) fn copy_scalar_keep_refs(doc: &Document, arr: &Object) -> Result<Object> {
    let out = doc.new_array();
    for i in 0..arr.array_len()? {
        let v = arr.array_get(i)?;
        if v.is_indirect() {
            out.array_push(&v)?;
        } else {
            out.array_push(&copy_scalar(doc, &v)?)?;
        }
    }
    Ok(out)
}

/// `a /Fit`-style default destination for a page.
pub(crate) fn fit_dest(doc: &Document, page: &Object) -> Result<Object> {
    let out = doc.new_array();
    out.array_push(page)?;
    out.array_push(&doc.new_name("Fit")?)?;
    Ok(out)
}

/// `base` when it is free, else the first free `base_k` (k >= 2).
pub(crate) fn unique_name(taken: &HashSet<Vec<u8>>, base: &[u8]) -> Vec<u8> {
    if !taken.contains(base) {
        return base.to_vec();
    }
    (2..)
        .map(|k| {
            let mut n = base.to_vec();
            n.extend_from_slice(format!("_{k}").as_bytes());
            n
        })
        .find(|n| !taken.contains(n))
        .expect("unbounded search")
}

/// Record every indirect node of the `/Names` machinery (the `/Names` dictionary and the nodes
/// of the destination tree) so name-tree edits are undoable.
pub(crate) fn touch_dest_tree(cx: &mut EditContext<'_>) -> Result<()> {
    let doc = cx.doc();
    let root = doc.root()?;
    cx.touch(&root)?;
    let names = root.dict_get("Names")?;
    if names.kind()? != ObjectKind::Dictionary {
        return Ok(());
    }
    if names.is_indirect() {
        cx.touch(&names)?;
    }
    let tree = names.dict_get("Dests")?;
    if tree.kind()? != ObjectKind::Dictionary {
        return Ok(());
    }
    let mut stack = vec![tree];
    let mut seen = HashSet::new();
    while let Some(n) = stack.pop() {
        if seen.len() > 1_000_000 {
            break;
        }
        if n.is_indirect() {
            if !seen.insert(n.id()) {
                continue;
            }
            cx.touch(&n)?;
        }
        let kids = n.dict_get("Kids")?;
        if kids.kind()? == ObjectKind::Array {
            stack.extend(kids.array_items()?);
        }
    }
    Ok(())
}
