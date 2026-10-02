//! The [`Model`]: owns a [`Document`], hands out cached typed views, and tracks which objects
//! each view read so that [`Model::invalidate`] can drop exactly the stale ones.

use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use papyrine_core::geom::Rect;
use papyrine_cos::{DecodeLevel, Document, ObjId, Object, ObjectKind, OpenOptions, Result};

use crate::text::{decode_text_lenient, decode_text_string};

/// Hard limits that keep hostile files from stalling a walk.
pub(crate) const MAX_DEPTH: usize = 64;
pub(crate) const MAX_NODES: usize = 500_000;
pub(crate) const MAX_ARRAY: usize = 2_000_000;

/// Pseudo object standing for "the page tree": bumped when any page or page-tree node changes.
pub(crate) const PAGE_TREE: ObjId = ObjId {
    num: u32::MAX,
    generation: 0,
};
/// Pseudo object for the trailer dictionary (a direct object with no id).
pub(crate) const TRAILER: ObjId = ObjId {
    num: u32::MAX - 1,
    generation: 0,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Key {
    PageList,
    Page(usize),
    PageLabels,
    Outlines,
    Form,
    Annots(usize),
    AnnotPageMap,
    NamedDests,
    Info,
    Catalog,
    Signatures,
    JavaScript,
}

struct Entry {
    deps: Vec<(ObjId, u64)>,
    value: Rc<dyn Any>,
}

/// Read-side document model. `!Send` like the [`Document`] it owns.
pub struct Model {
    doc: Document,
    gens: RefCell<HashMap<ObjId, u64>>,
    tree_nodes: RefCell<HashSet<ObjId>>,
    cache: RefCell<HashMap<Key, Entry>>,
    frames: RefCell<Vec<Vec<ObjId>>>,
    diagnostics: RefCell<Vec<String>>,
    invalidations: std::cell::Cell<u64>,
}

impl Model {
    pub fn new(doc: Document) -> Model {
        Model {
            doc,
            gens: RefCell::default(),
            tree_nodes: RefCell::default(),
            cache: RefCell::default(),
            frames: RefCell::default(),
            diagnostics: RefCell::default(),
            invalidations: std::cell::Cell::new(0),
        }
    }

    pub fn open_path(path: impl AsRef<Path>, opts: &OpenOptions) -> Result<Model> {
        Ok(Model::new(Document::open_path(path, opts)?))
    }

    pub fn open_bytes(data: impl AsRef<[u8]> + 'static, opts: &OpenOptions) -> Result<Model> {
        Ok(Model::new(Document::open_bytes(data, opts)?))
    }

    pub fn document(&self) -> &Document {
        &self.doc
    }

    pub fn into_document(self) -> Document {
        self.doc
    }

    /// Non-fatal oddities met while building views (cycles, truncated walks, bad values).
    pub fn diagnostics(&self) -> Vec<String> {
        self.diagnostics.borrow().clone()
    }

    pub(crate) fn note(&self, msg: impl Into<String>) {
        let mut d = self.diagnostics.borrow_mut();
        if d.len() < 1000 {
            d.push(msg.into());
        }
    }

    // ----- generation counters -------------------------------------------------------------

    /// Current generation of an object (0 until it is first invalidated).
    pub fn generation(&self, id: ObjId) -> u64 {
        self.gens.borrow().get(&id).copied().unwrap_or(0)
    }

    /// Total number of object invalidations so far; a cheap "anything changed" token.
    pub fn epoch(&self) -> u64 {
        self.invalidations.get()
    }

    /// Bump the generation of each object, dropping every cached view that read any of them.
    /// The journal calls this with the touched and created object ids after each command.
    pub fn invalidate(&self, ids: &[ObjId]) {
        {
            let mut gens = self.gens.borrow_mut();
            let tree = self.tree_nodes.borrow();
            let mut tree_hit = false;
            for &id in ids {
                *gens.entry(id).or_insert(0) += 1;
                tree_hit |= tree.contains(&id);
            }
            if tree_hit {
                *gens.entry(PAGE_TREE).or_insert(0) += 1;
            }
        }
        self.invalidations
            .set(self.invalidations.get() + ids.len() as u64);
        self.sweep();
    }

    /// Invalidate every cached view (for example after reloading the document).
    pub fn invalidate_all(&self) {
        self.cache.borrow_mut().clear();
        self.tree_nodes.borrow_mut().clear();
        self.invalidations.set(self.invalidations.get() + 1);
    }

    /// The page tree changed in a way the model cannot see (pages added or removed through the
    /// cos API without naming ids).
    pub fn invalidate_pages(&self) {
        *self.gens.borrow_mut().entry(PAGE_TREE).or_insert(0) += 1;
        self.invalidations.set(self.invalidations.get() + 1);
        self.sweep();
    }

    fn sweep(&self) {
        let gens = self.gens.borrow();
        self.cache.borrow_mut().retain(|_, e| {
            e.deps
                .iter()
                .all(|(id, g)| gens.get(id).copied().unwrap_or(0) == *g)
        });
    }

    /// Number of cached views (diagnostic; used by tests).
    pub fn cached_view_count(&self) -> usize {
        self.cache.borrow().len()
    }

    // ----- cache with dependency recording -------------------------------------------------

    pub(crate) fn cached<T: 'static>(
        &self,
        key: Key,
        build: impl FnOnce(&Model) -> Result<T>,
    ) -> Result<Rc<T>> {
        let hit = {
            let cache = self.cache.borrow();
            cache.get(&key).map(|e| (e.value.clone(), e.deps.clone()))
        };
        if let Some((value, deps)) = hit {
            self.extend_frame(deps.iter().map(|(id, _)| *id));
            if let Ok(v) = value.downcast::<T>() {
                return Ok(v);
            }
        }
        self.frames.borrow_mut().push(Vec::new());
        let built = build(self);
        let mut recorded = self.frames.borrow_mut().pop().unwrap_or_default();
        let value = Rc::new(built?);
        recorded.sort_unstable();
        recorded.dedup();
        let deps: Vec<(ObjId, u64)> = recorded
            .iter()
            .map(|&id| (id, self.generation(id)))
            .collect();
        self.extend_frame(recorded.iter().copied());
        self.cache.borrow_mut().insert(
            key,
            Entry {
                deps,
                value: value.clone(),
            },
        );
        Ok(value)
    }

    fn extend_frame(&self, ids: impl Iterator<Item = ObjId>) {
        if let Some(top) = self.frames.borrow_mut().last_mut() {
            top.extend(ids);
        }
    }

    /// Replace what the view being built depends on (for views that summarize large structures
    /// and are invalidated through a pseudo object instead).
    pub(crate) fn set_frame_deps(&self, ids: Vec<ObjId>) {
        if let Some(top) = self.frames.borrow_mut().last_mut() {
            *top = ids;
        }
    }

    pub(crate) fn dep_id(&self, id: ObjId) {
        if let Some(top) = self.frames.borrow_mut().last_mut() {
            top.push(id);
        }
    }

    /// Record that the view being built read `o` (when it is indirect).
    pub(crate) fn dep(&self, o: &Object) {
        if let Some(id) = o.id() {
            self.dep_id(id);
        }
    }

    pub(crate) fn register_tree_nodes(&self, ids: HashSet<ObjId>) {
        *self.tree_nodes.borrow_mut() = ids;
    }

    // ----- lenient accessors (None instead of errors; every indirect hop is recorded) -------

    pub(crate) fn root(&self) -> Option<Object> {
        let r = self.doc.root().ok()?;
        self.dep(&r);
        (self.kind(&r) == ObjectKind::Dictionary).then_some(r)
    }

    pub(crate) fn trailer(&self) -> Option<Object> {
        self.dep_id(TRAILER);
        self.doc.trailer().ok()
    }

    pub(crate) fn kind(&self, o: &Object) -> ObjectKind {
        o.kind().unwrap_or(ObjectKind::Null)
    }

    pub(crate) fn is_dict(&self, o: &Object) -> bool {
        matches!(self.kind(o), ObjectKind::Dictionary | ObjectKind::Stream)
    }

    /// `o[key]`, or `None` when `o` is not a dictionary, the key is absent, or the value is null.
    pub(crate) fn get(&self, o: &Object, key: &str) -> Option<Object> {
        self.dep(o);
        let v = o.dict_get(key).ok()?;
        self.dep(&v);
        (self.kind(&v) != ObjectKind::Null).then_some(v)
    }

    pub(crate) fn dict_keys(&self, o: &Object) -> Vec<String> {
        self.dep(o);
        if !self.is_dict(o) {
            return Vec::new();
        }
        o.dict_keys()
            .map(|ks| {
                ks.into_iter()
                    .map(|k| String::from_utf8_lossy(&k).into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn name(&self, o: &Object) -> Option<String> {
        (self.kind(o) == ObjectKind::Name)
            .then(|| {
                o.name()
                    .ok()
                    .map(|n| String::from_utf8_lossy(&n).into_owned())
            })
            .flatten()
    }

    pub(crate) fn name_of(&self, o: &Object, key: &str) -> Option<String> {
        self.get(o, key).and_then(|v| self.name(&v))
    }

    pub(crate) fn int(&self, o: &Object) -> Option<i64> {
        match self.kind(o) {
            ObjectKind::Integer => o.as_int().ok(),
            ObjectKind::Real => o.as_f64().ok().filter(|f| f.is_finite()).map(|f| f as i64),
            _ => None,
        }
    }

    pub(crate) fn int_of(&self, o: &Object, key: &str) -> Option<i64> {
        self.get(o, key).and_then(|v| self.int(&v))
    }

    pub(crate) fn num(&self, o: &Object) -> Option<f64> {
        match self.kind(o) {
            ObjectKind::Integer | ObjectKind::Real => o.as_f64().ok().filter(|f| f.is_finite()),
            _ => None,
        }
    }

    pub(crate) fn num_of(&self, o: &Object, key: &str) -> Option<f64> {
        self.get(o, key).and_then(|v| self.num(&v))
    }

    pub(crate) fn boolean(&self, o: &Object) -> Option<bool> {
        (self.kind(o) == ObjectKind::Bool)
            .then(|| o.as_bool().ok())
            .flatten()
    }

    pub(crate) fn bool_of(&self, o: &Object, key: &str) -> Option<bool> {
        self.get(o, key).and_then(|v| self.boolean(&v))
    }

    /// Raw bytes of a string object.
    pub(crate) fn bytes(&self, o: &Object) -> Option<Vec<u8>> {
        (self.kind(o) == ObjectKind::String)
            .then(|| o.string().ok())
            .flatten()
    }

    /// Decoded text string (strings only).
    pub(crate) fn text(&self, o: &Object) -> Option<String> {
        self.bytes(o).map(|b| decode_text_string(&b))
    }

    pub(crate) fn text_of(&self, o: &Object, key: &str) -> Option<String> {
        self.get(o, key).and_then(|v| self.text(&v))
    }

    /// Text from a string, or from a text stream (as used for JavaScript and rich text).
    pub(crate) fn text_or_stream(&self, o: &Object) -> Option<String> {
        match self.kind(o) {
            ObjectKind::String => self.bytes(o).map(|b| decode_text_lenient(&b)),
            ObjectKind::Stream => {
                let data = o.stream_decoded(DecodeLevel::Generalized).ok()?;
                Some(decode_text_lenient(data.as_slice()))
            }
            _ => None,
        }
    }

    /// Array elements (capped); empty when `o` is not an array.
    pub(crate) fn items(&self, o: &Object) -> Vec<Object> {
        self.dep(o);
        if self.kind(o) != ObjectKind::Array {
            return Vec::new();
        }
        let n = o.array_len().unwrap_or(0).min(MAX_ARRAY);
        let mut out = Vec::with_capacity(n.min(4096));
        for i in 0..n {
            if let Ok(e) = o.array_get(i) {
                self.dep(&e);
                out.push(e);
            }
        }
        out
    }

    pub(crate) fn items_of(&self, o: &Object, key: &str) -> Vec<Object> {
        self.get(o, key).map(|a| self.items(&a)).unwrap_or_default()
    }

    /// All numeric elements of an array (non-numbers are skipped).
    pub(crate) fn numbers(&self, o: &Object) -> Vec<f64> {
        self.items(o).iter().filter_map(|e| self.num(e)).collect()
    }

    pub(crate) fn numbers_of(&self, o: &Object, key: &str) -> Vec<f64> {
        self.get(o, key)
            .map(|a| self.numbers(&a))
            .unwrap_or_default()
    }

    /// A four-number array as a normalized rectangle.
    pub(crate) fn rect(&self, o: &Object) -> Option<Rect> {
        let items = self.items(o);
        if items.len() < 4 {
            return None;
        }
        let mut n = [0.0; 4];
        for (slot, e) in n.iter_mut().zip(&items) {
            *slot = self.num(e)?;
        }
        Some(Rect::new(n[0], n[1], n[2], n[3]).normalized())
    }

    pub(crate) fn rect_of(&self, o: &Object, key: &str) -> Option<Rect> {
        self.get(o, key).and_then(|v| self.rect(&v))
    }

    pub(crate) fn id_of(&self, o: &Object, key: &str) -> Option<ObjId> {
        self.get(o, key).and_then(|v| v.id())
    }

    /// Value of an inheritable key: the node itself, then its `/Parent` chain.
    pub(crate) fn inherited(&self, o: &Object, key: &str) -> Option<Object> {
        let mut node = o.clone();
        let mut seen = HashSet::new();
        for _ in 0..MAX_DEPTH {
            if let Some(v) = self.get(&node, key) {
                return Some(v);
            }
            let parent = self.get(&node, "Parent")?;
            if let Some(id) = parent.id()
                && !seen.insert(id)
            {
                self.note(format!("parent loop at {id}"));
                return None;
            }
            if !self.is_dict(&parent) {
                return None;
            }
            node = parent;
        }
        None
    }
}
