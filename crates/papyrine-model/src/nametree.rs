//! Name trees and number trees (PDF 7.9.6 and 7.9.7), walked defensively.

use std::collections::HashSet;

use papyrine_cos::Object;

use crate::model::{MAX_DEPTH, MAX_NODES, Model};

impl Model {
    /// Every `(name bytes, value)` leaf of a name tree, in tree order.
    pub(crate) fn name_tree_entries(&self, root: &Object) -> Vec<(Vec<u8>, Object)> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.walk_tree(root, 0, &mut seen, &mut |m, node| {
            let items = m.items_of(node, "Names");
            for pair in items.as_chunks::<2>().0.iter() {
                if let Some(k) = m.bytes(&pair[0]) {
                    out.push((k, pair[1].clone()));
                }
            }
            out.len() < MAX_NODES
        });
        out
    }

    /// Every `(number, value)` leaf of a number tree.
    pub(crate) fn number_tree_entries(&self, root: &Object) -> Vec<(i64, Object)> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        self.walk_tree(root, 0, &mut seen, &mut |m, node| {
            let items = m.items_of(node, "Nums");
            for pair in items.as_chunks::<2>().0.iter() {
                if let Some(k) = m.int(&pair[0]) {
                    out.push((k, pair[1].clone()));
                }
            }
            out.len() < MAX_NODES
        });
        out
    }

    fn walk_tree(
        &self,
        node: &Object,
        depth: usize,
        seen: &mut HashSet<papyrine_cos::ObjId>,
        leaf: &mut dyn FnMut(&Model, &Object) -> bool,
    ) -> bool {
        if depth > MAX_DEPTH || !self.is_dict(node) {
            return true;
        }
        if let Some(id) = node.id()
            && !seen.insert(id)
        {
            self.note(format!("tree cycle at {id}"));
            return true;
        }
        if !leaf(self, node) {
            return false;
        }
        for kid in self.items_of(node, "Kids") {
            if !self.walk_tree(&kid, depth + 1, seen, leaf) {
                return false;
            }
        }
        true
    }

    /// Look up one key in a name tree, pruning subtrees with `/Limits`.
    pub(crate) fn name_tree_lookup(&self, root: &Object, key: &[u8]) -> Option<Object> {
        let mut seen = HashSet::new();
        self.lookup_in(root, key, 0, &mut seen)
    }

    fn lookup_in(
        &self,
        node: &Object,
        key: &[u8],
        depth: usize,
        seen: &mut HashSet<papyrine_cos::ObjId>,
    ) -> Option<Object> {
        if depth > MAX_DEPTH || !self.is_dict(node) {
            return None;
        }
        if let Some(id) = node.id()
            && !seen.insert(id)
        {
            return None;
        }
        let names = self.items_of(node, "Names");
        for pair in names.as_chunks::<2>().0.iter() {
            if self.bytes(&pair[0]).as_deref() == Some(key) {
                return Some(pair[1].clone());
            }
        }
        for kid in self.items_of(node, "Kids") {
            if let Some(limits) = self.get(&kid, "Limits") {
                let l = self.items(&limits);
                if l.len() >= 2
                    && let (Some(lo), Some(hi)) = (self.bytes(&l[0]), self.bytes(&l[1]))
                    && (key < lo.as_slice() || key > hi.as_slice())
                {
                    continue;
                }
            }
            if let Some(v) = self.lookup_in(&kid, key, depth + 1, seen) {
                return Some(v);
            }
        }
        None
    }
}
