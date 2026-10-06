//! Destinations: named-destination lookup (name tree and the legacy `/Dests` dictionary),
//! explicit destination arrays, and the `/Dest` / `/A /D` slots of annotations and bookmarks.

use std::collections::{BTreeMap, HashSet};

use papyrine_cos::{Document, ObjId, Object, ObjectKind};

use crate::context::EditContext;
use crate::error::Result;
use crate::text::decode_text_string;

/// A name as compared by viewers: string keys are PDF text strings, so `(abc)` in PDFDocEncoding,
/// UTF-16 and (non-conforming but common) bare UTF-8 are the same name; names (legacy `/Dests` keys, `/Dest /foo`) are used as is.
pub fn norm_string(raw: &[u8]) -> Vec<u8> {
    if raw.starts_with(&[0xFE, 0xFF])
        || raw.starts_with(&[0xFF, 0xFE])
        || raw.starts_with(&[0xEF, 0xBB, 0xBF])
    {
        return decode_text_string(raw).into_bytes();
    }
    // Producers such as InDesign store bare UTF-8 in name-tree keys; a PDFDocEncoded string with
    // high bytes is almost never valid UTF-8 by accident, so accept it as UTF-8.
    if raw.is_ascii() || std::str::from_utf8(raw).is_ok() {
        return raw.to_vec();
    }
    raw.iter()
        .map(|&b| pdfdoc_char(b))
        .collect::<String>()
        .into_bytes()
}

/// A key as qpdf orders it: PDF text-string decoding (UTF-16 or UTF-8 with BOM, else every byte
/// as PDFDocEncoding), compared as UTF-8.
fn qpdf_key(raw: &[u8]) -> Vec<u8> {
    if raw.starts_with(&[0xFE, 0xFF])
        || raw.starts_with(&[0xFF, 0xFE])
        || raw.starts_with(&[0xEF, 0xBB, 0xBF])
    {
        return decode_text_string(raw).into_bytes();
    }
    raw.iter()
        .map(|&b| pdfdoc_char(b))
        .collect::<String>()
        .into_bytes()
}

/// PDFDocEncoding (PDF 32000 annex D.2): Latin-1 apart from the punctuation block 0x80-0xA0
/// and a few accents at 0x18-0x1F.
fn pdfdoc_char(b: u8) -> char {
    const HIGH: [char; 33] = [
        '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}',
        '\u{2044}', '\u{2039}', '\u{203A}', '\u{2212}', '\u{2030}', '\u{201E}', '\u{201C}',
        '\u{201D}', '\u{2018}', '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}', '\u{FB02}',
        '\u{0141}', '\u{0152}', '\u{0160}', '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}',
        '\u{0153}', '\u{0161}', '\u{017E}', '\u{FFFD}', '\u{20AC}',
    ];
    const LOW: [char; 8] = [
        '\u{02D8}', '\u{02C7}', '\u{02C6}', '\u{02D9}', '\u{02DD}', '\u{02DB}', '\u{02DA}',
        '\u{02DC}',
    ];
    match b {
        0x18..=0x1F => LOW[(b - 0x18) as usize],
        0x80..=0xA0 => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

/// Every `(key, value)` of a name tree, found by walking `/Kids` and `/Names` rather than by
/// searching, so trees whose keys are not in sorted order (common in real files) are read in
/// full. Bounded in size and depth against hostile files.
pub fn name_tree_entries(root: &Object) -> Result<Vec<(Vec<u8>, Object)>> {
    const MAX_NODES: usize = 1_000_000;
    let mut out = vec![];
    let mut stack = vec![(root.clone(), 0usize)];
    let mut seen: HashSet<ObjId> = HashSet::new();
    let mut nodes = 0;
    while let Some((n, depth)) = stack.pop() {
        nodes += 1;
        if depth > 32 || nodes > MAX_NODES || n.kind()? != ObjectKind::Dictionary {
            continue;
        }
        if let Some(id) = n.id()
            && !seen.insert(id)
        {
            continue;
        }
        let names = n.dict_get("Names")?;
        if names.kind()? == ObjectKind::Array {
            let items = names.array_items()?;
            for pair in items.as_chunks::<2>().0 {
                if pair[0].kind()? == ObjectKind::String {
                    out.push((pair[0].string()?, pair[1].clone()));
                }
            }
        }
        let kids = n.dict_get("Kids")?;
        if kids.kind()? == ObjectKind::Array {
            // Reverse so the stack pops them in document order.
            for k in kids.array_items()?.into_iter().rev() {
                stack.push((k, depth + 1));
            }
        }
    }
    Ok(out)
}

/// All named destinations of a document, keyed by normalized name (see [`norm_string`]). Names
/// from the `/Names /Dests` tree win over the legacy `/Dests` dictionary.
pub(crate) struct DestMap {
    /// normalized name -> (key bytes as stored in the tree, value)
    tree: BTreeMap<Vec<u8>, (Vec<u8>, Object)>,
    legacy: BTreeMap<Vec<u8>, Object>,
}

impl DestMap {
    pub fn collect(doc: &Document) -> Result<DestMap> {
        let mut legacy = BTreeMap::new();
        let mut tree = BTreeMap::new();
        let root = doc.root()?;
        let l = root.dict_get("Dests")?;
        if l.kind()? == ObjectKind::Dictionary {
            for k in l.dict_keys()? {
                legacy.insert(k.clone(), l.dict_get(&k)?);
            }
        }
        if let Some(t) = doc.catalog_name_tree("Dests", false)? {
            for (k, v) in name_tree_entries(&t)? {
                tree.insert(norm_string(&k), (k, v));
            }
        }
        Ok(DestMap { tree, legacy })
    }

    pub fn names(&self) -> HashSet<Vec<u8>> {
        self.tree
            .keys()
            .chain(self.legacy.keys())
            .cloned()
            .collect()
    }

    fn value(&self, name: &[u8]) -> Option<&Object> {
        self.tree
            .get(name)
            .map(|(_, v)| v)
            .or_else(|| self.legacy.get(name))
    }

    /// The destination array for a normalized name.
    pub fn array(&self, name: &[u8]) -> Result<Option<Object>> {
        match self.value(name) {
            Some(v) => dest_array(v),
            None => Ok(None),
        }
    }

    /// Every name once, with its value.
    pub fn iter(&self) -> impl Iterator<Item = (&Vec<u8>, &Object)> {
        self.tree.iter().map(|(k, (_, v))| (k, v)).chain(
            self.legacy
                .iter()
                .filter(|(k, _)| !self.tree.contains_key(*k)),
        )
    }

    /// The key bytes the tree stores for `name`, if the tree has it.
    pub fn tree_key(&self, name: &[u8]) -> Option<&[u8]> {
        self.tree.get(name).map(|(raw, _)| raw.as_slice())
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
            ObjectKind::String => Some(Target::Named(norm_string(&o.string()?))),
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

/// Record what [`write_dest_tree`] changes: the catalog and the `/Names` dictionary.
pub(crate) fn touch_names(cx: &mut EditContext<'_>) -> Result<()> {
    let root = cx.doc().root()?;
    cx.touch(&root)?;
    let names = root.dict_get("Names")?;
    if names.kind()? == ObjectKind::Dictionary && names.is_indirect() {
        cx.touch(&names)?;
    }
    Ok(())
}

const LEAF: usize = 64;

/// Replace the `/Names /Dests` tree by a fresh one holding `entries` (key bytes exactly as they
/// are to be stored). qpdf's own name-tree API compares keys as decoded text, which
/// mis-handles keys stored as bare UTF-8 (common in real files), so the tree is built here:
/// keys ordered the way qpdf validates them, leaves of at most 64 pairs. The old nodes are left
/// untouched (they become unreachable), which keeps undo simple.
pub(crate) fn write_dest_tree(
    cx: &mut EditContext<'_>,
    mut entries: Vec<(Vec<u8>, Object)>,
) -> Result<()> {
    touch_names(cx)?;
    let doc = cx.doc();
    let root = doc.root()?;
    let mut names = root.dict_get("Names")?;
    if names.kind()? != ObjectKind::Dictionary {
        if entries.is_empty() {
            return Ok(());
        }
        root.dict_set("Names", &doc.new_dict())?;
        names = root.dict_get("Names")?;
    }
    if entries.is_empty() {
        names.dict_remove("Dests")?;
        return Ok(());
    }
    // qpdf validates trees with keys ordered by their decoded text (not raw bytes).
    entries.sort_by_cached_key(|e| qpdf_key(&e.0));
    let leaf = |chunk: &[(Vec<u8>, Object)]| -> Result<Object> {
        let d = doc.new_dict();
        let arr = doc.new_array();
        for (k, v) in chunk {
            arr.array_push(&doc.new_string(k)?)?;
            arr.array_push(v)?;
        }
        d.dict_set("Names", &arr)?;
        Ok(d)
    };
    if entries.len() <= LEAF {
        names.dict_set("Dests", &leaf(&entries)?)?;
        return Ok(());
    }
    let kids = doc.new_array();
    for chunk in entries.chunks(LEAF) {
        let l = leaf(chunk)?;
        let limits = doc.new_array();
        limits.array_push(&doc.new_string(&chunk[0].0)?)?;
        limits.array_push(&doc.new_string(&chunk[chunk.len() - 1].0)?)?;
        l.dict_set("Limits", &limits)?;
        kids.array_push(&doc.make_indirect(&l)?)?;
    }
    let tree = doc.new_dict();
    tree.dict_set("Kids", &kids)?;
    names.dict_set("Dests", &tree)?;
    Ok(())
}

/// The destination tree's entries exactly as stored.
pub(crate) fn raw_entries(doc: &Document) -> Result<Vec<(Vec<u8>, Object)>> {
    match doc.catalog_name_tree("Dests", false)? {
        Some(t) => name_tree_entries(&t),
        None => Ok(vec![]),
    }
}
