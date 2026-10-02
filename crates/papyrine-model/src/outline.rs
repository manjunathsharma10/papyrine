//! Document outline (bookmarks).

use std::collections::HashSet;
use std::rc::Rc;

use papyrine_cos::{ObjId, Object, Result};

use crate::dest::{Action, ActionKind, Destination};
use crate::model::{Key, MAX_DEPTH, MAX_NODES, Model};

#[derive(Debug, Clone)]
pub struct OutlineItem {
    pub id: ObjId,
    pub title: String,
    /// `/Dest`, or the destination of a `/A` GoTo action.
    pub dest: Option<Destination>,
    /// `/A`, when present.
    pub action: Option<Action>,
    /// `/Count > 0`: the item is shown expanded.
    pub open: bool,
    /// Raw `/Count` (negative: closed, magnitude = descendants).
    pub count: i64,
    /// `/C` as RGB in 0..=1.
    pub color: Option<[f64; 3]>,
    pub bold: bool,
    pub italic: bool,
    pub children: Vec<OutlineItem>,
}

impl OutlineItem {
    /// Number of items in this subtree, itself included.
    pub fn subtree_len(&self) -> usize {
        1 + self
            .children
            .iter()
            .map(OutlineItem::subtree_len)
            .sum::<usize>()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Outlines {
    pub items: Vec<OutlineItem>,
    /// The walk stopped early (depth, size or cycle guard).
    pub truncated: bool,
}

impl Outlines {
    pub fn total(&self) -> usize {
        self.items.iter().map(OutlineItem::subtree_len).sum()
    }
}

struct Walk {
    seen: HashSet<ObjId>,
    nodes: usize,
    truncated: bool,
}

impl Model {
    pub fn outlines(&self) -> Result<Rc<Outlines>> {
        self.cached(Key::Outlines, |m| {
            let mut out = Outlines::default();
            let Some(root) = m.root() else {
                return Ok(out);
            };
            let Some(ol) = m.get(&root, "Outlines") else {
                return Ok(out);
            };
            let mut w = Walk {
                seen: HashSet::new(),
                nodes: 0,
                truncated: false,
            };
            if let Some(id) = ol.id() {
                w.seen.insert(id);
            }
            out.items = m.outline_children(&ol, 0, &mut w);
            out.truncated = w.truncated;
            Ok(out)
        })
    }

    fn outline_children(&self, parent: &Object, depth: usize, w: &mut Walk) -> Vec<OutlineItem> {
        let mut items = Vec::new();
        if depth >= MAX_DEPTH {
            w.truncated = true;
            return items;
        }
        let mut cur = self.get(parent, "First");
        while let Some(node) = cur {
            if !self.is_dict(&node) {
                break;
            }
            if let Some(id) = node.id()
                && !w.seen.insert(id)
            {
                self.note(format!("outline cycle at {id}"));
                w.truncated = true;
                break;
            }
            w.nodes += 1;
            if w.nodes > MAX_NODES {
                w.truncated = true;
                break;
            }
            let mut item = self.outline_item(&node);
            item.children = self.outline_children(&node, depth + 1, w);
            items.push(item);
            cur = self.get(&node, "Next");
        }
        items
    }

    fn outline_item(&self, node: &Object) -> OutlineItem {
        let action = self.get(node, "A").and_then(|a| self.parse_action(&a));
        let dest = self
            .get(node, "Dest")
            .and_then(|d| self.parse_destination(&d))
            .or_else(|| {
                action.as_ref().and_then(|a| match &a.kind {
                    ActionKind::GoTo(d) => Some(d.clone()),
                    _ => None,
                })
            });
        let count = self.int_of(node, "Count").unwrap_or(0);
        let flags = self.int_of(node, "F").unwrap_or(0);
        let color = self.get(node, "C").and_then(|c| {
            let n = self.numbers(&c);
            (n.len() == 3).then(|| [n[0], n[1], n[2]])
        });
        OutlineItem {
            id: node.id().unwrap_or(ObjId::new(0, 0)),
            title: self.text_of(node, "Title").unwrap_or_default(),
            dest,
            action,
            open: count > 0,
            count,
            color,
            bold: flags & 2 != 0,
            italic: flags & 1 != 0,
            children: Vec::new(),
        }
    }
}
