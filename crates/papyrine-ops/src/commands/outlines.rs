//! Bookmarks: reading a source outline, rebuilding the part that targets copied pages inside the
//! destination, and neutralising bookmarks that point at deleted pages.

use std::collections::{HashMap, HashSet};

use papyrine_cos::{Document, ObjId, Object, ObjectKind};
use serde::{Deserialize, Serialize};

use crate::commands::dests::{self, DestMap};
use crate::context::EditContext;
use crate::error::Result;

const MAX_ITEMS: usize = 200_000;
const MAX_DEPTH: usize = 64;

/// What to do with the source's bookmarks when pages are imported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutlineMode {
    /// Leave them behind.
    None,
    /// Add the source's top-level bookmarks (those that lead to imported pages) to the
    /// destination's top level.
    #[default]
    Merged,
    /// Add one top-level bookmark for the whole import, with the source's bookmarks below it.
    PerFile,
}

/// One bookmark of a source outline.
pub(crate) struct Node {
    pub obj: Object,
    pub kids: Vec<Node>,
}

/// The source's bookmark forest (bounded in size and depth against hostile files).
pub(crate) fn read_forest(doc: &Document) -> Result<Vec<Node>> {
    let root = doc.root()?.dict_get("Outlines")?;
    if root.kind()? != ObjectKind::Dictionary {
        return Ok(vec![]);
    }
    let mut budget = MAX_ITEMS;
    let mut seen = HashSet::new();
    read_siblings(&root.dict_get("First")?, 0, &mut budget, &mut seen)
}

fn read_siblings(
    first: &Object,
    depth: usize,
    budget: &mut usize,
    seen: &mut HashSet<ObjId>,
) -> Result<Vec<Node>> {
    let mut out = vec![];
    let mut cur = first.clone();
    while cur.kind()? == ObjectKind::Dictionary && depth <= MAX_DEPTH && *budget > 0 {
        if let Some(id) = cur.id()
            && !seen.insert(id)
        {
            break;
        }
        *budget -= 1;
        let kids = read_siblings(&cur.dict_get("First")?, depth + 1, budget, seen)?;
        let next = cur.dict_get("Next")?;
        out.push(Node { obj: cur, kids });
        cur = next;
    }
    Ok(out)
}

/// Everything needed to rebuild bookmarks inside the destination.
pub(crate) struct Rebuild<'a> {
    pub dest: &'a Document,
    pub src_names: &'a DestMap,
    /// Source page object id -> page copy in the destination.
    pub page_map: &'a HashMap<ObjId, Object>,
    /// Every source page was imported: bookmarks without a page target (URIs) come along.
    pub full: bool,
    /// Bookmarks created so far (for reports).
    pub created: std::cell::Cell<usize>,
}

impl Rebuild<'_> {
    fn dest_for(&self, n: &Node) -> Result<Option<(Object, Object)>> {
        let Some((_, t)) = dests::holder_dest(&n.obj)? else {
            return Ok(None);
        };
        let Some(arr) = dests::resolve(self.src_names, &t)? else {
            return Ok(None);
        };
        let Some(pid) = dests::dest_page(&arr)? else {
            return Ok(None);
        };
        Ok(self.page_map.get(&pid).map(|np| (np.clone(), arr)))
    }

    pub fn any_kept(&self, nodes: &[Node]) -> Result<bool> {
        for n in nodes {
            if self.keep(n)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn keep(&self, n: &Node) -> Result<bool> {
        if self.dest_for(n)?.is_some() {
            return Ok(true);
        }
        for k in &n.kids {
            if self.keep(k)? {
                return Ok(true);
            }
        }
        // Items that lead nowhere in the document (web links, no destination) belong to
        // the whole file, not to a page range.
        Ok(self.full && dests::holder_dest(&n.obj)?.is_none())
    }

    /// Build the destination bookmarks for the kept part of `nodes`, linked as siblings under
    /// `parent`. Returns them in order plus the visible-descendant total of the group.
    pub fn build(&self, nodes: &[Node], parent: &Object) -> Result<Vec<Object>> {
        let doc = self.dest;
        let mut items: Vec<Object> = vec![];
        for n in nodes {
            if !self.keep(n)? {
                continue;
            }
            let it = doc.make_indirect(&doc.new_dict())?;
            self.created.set(self.created.get() + 1);
            let title = n.obj.dict_get("Title")?;
            let title_bytes = if title.kind()? == ObjectKind::String {
                title.string()?
            } else {
                vec![]
            };
            it.dict_set("Title", &doc.new_string(&title_bytes)?)?;
            it.dict_set("Parent", parent)?;
            if let Some((np, arr)) = self.dest_for(n)? {
                it.dict_set("Dest", &dests::retarget(doc, &arr, &np)?)?;
            } else {
                let a = n.obj.dict_get("A")?;
                if a.kind()? == ObjectKind::Dictionary
                    && a.dict_get("S")?.name().is_ok_and(|s| s == b"URI")
                {
                    it.dict_set("A", &dests::copy_scalar(doc, &a)?)?;
                }
            }
            for key in ["C", "F"] {
                let v = n.obj.dict_get(key)?;
                if matches!(v.kind()?, ObjectKind::Array | ObjectKind::Integer) {
                    it.dict_set(key, &dests::copy_scalar(doc, &v)?)?;
                }
            }
            let kids = self.build(&n.kids, &it)?;
            if let (Some(first), Some(last)) = (kids.first(), kids.last()) {
                // A heading whose own page was not imported leads to its first section instead.
                if !it.dict_has("Dest")? && !it.dict_has("A")? {
                    let d = first.dict_get("Dest")?;
                    if d.kind()? == ObjectKind::Array {
                        it.dict_set("Dest", &dests::copy_scalar_keep_refs(doc, &d)?)?;
                    }
                }
                it.dict_set("First", first)?;
                it.dict_set("Last", last)?;
                let visible = visible_total(&kids)?;
                let open = n.obj.dict_get("Count")?.as_int().unwrap_or(0) > 0;
                it.dict_set("Count", &doc.new_int(if open { visible } else { -visible }))?;
            }
            items.push(it);
        }
        link_siblings(&items)?;
        Ok(items)
    }
}

fn link_siblings(items: &[Object]) -> Result<()> {
    for w in items.windows(2) {
        w[0].dict_set("Next", &w[1])?;
        w[1].dict_set("Prev", &w[0])?;
    }
    Ok(())
}

/// Number of bookmarks shown below a parent whose children are `items` (each item plus its
/// visible descendants).
fn visible_total(items: &[Object]) -> Result<i64> {
    let mut n = 0;
    for i in items {
        n += 1 + i.dict_get("Count")?.as_int().unwrap_or(0).max(0);
    }
    Ok(n)
}

/// The destination's `/Outlines` root, created when missing. The catalog is already recorded.
fn ensure_root(cx: &mut EditContext<'_>) -> Result<Object> {
    let doc = cx.doc();
    let root = doc.root()?;
    cx.touch(&root)?;
    let mut o = root.dict_get("Outlines")?;
    if o.kind()? != ObjectKind::Dictionary {
        let d = doc.parse_object("<< /Type /Outlines >>")?;
        o = doc.make_indirect(&d)?;
        root.dict_set("Outlines", &o)?;
    } else {
        cx.touch(&o)?;
    }
    Ok(o)
}

/// Where to put the group within the destination's top level.
pub(crate) enum Place {
    End,
    /// Before the first existing top-level bookmark that leads to a page at or after this
    /// index (counted after the import).
    BeforePage(usize),
}

/// The outline root to build under (created if needed) before [`attach`].
pub(crate) fn root_for_build(cx: &mut EditContext<'_>) -> Result<Object> {
    ensure_root(cx)
}

/// Link a group of sibling bookmarks (already parented to `root`) into the top level.
pub(crate) fn attach(
    cx: &mut EditContext<'_>,
    root: &Object,
    group: &[Object],
    place: Place,
) -> Result<()> {
    let (Some(g_first), Some(g_last)) = (group.first(), group.last()) else {
        return Ok(());
    };
    let doc = cx.doc();
    let before = match place {
        Place::End => None,
        Place::BeforePage(limit) => {
            let names = DestMap::collect(doc)?;
            let mut cur = root.dict_get("First")?;
            let mut found = None;
            let mut guard = 0;
            while cur.kind()? == ObjectKind::Dictionary && guard < MAX_ITEMS {
                guard += 1;
                if let Some((_, t)) = dests::holder_dest(&cur)?
                    && let Some(arr) = dests::resolve(&names, &t)?
                    && let Some(pid) = dests::dest_page(&arr)?
                    && let Ok(idx) = doc.find_page(&doc.object(pid)?)
                    && idx >= limit
                {
                    found = Some(cur.clone());
                    break;
                }
                cur = cur.dict_get("Next")?;
            }
            found
        }
    };
    match before {
        Some(b) => {
            cx.touch(&b)?;
            let prev = b.dict_get("Prev")?;
            if prev.kind()? == ObjectKind::Dictionary {
                cx.touch(&prev)?;
                prev.dict_set("Next", g_first)?;
                g_first.dict_set("Prev", &prev)?;
            } else {
                root.dict_set("First", g_first)?;
            }
            b.dict_set("Prev", g_last)?;
            g_last.dict_set("Next", &b)?;
        }
        None => {
            let last = root.dict_get("Last")?;
            if last.kind()? == ObjectKind::Dictionary {
                cx.touch(&last)?;
                last.dict_set("Next", g_first)?;
                g_first.dict_set("Prev", &last)?;
            } else {
                root.dict_set("First", g_first)?;
            }
            root.dict_set("Last", g_last)?;
        }
    }
    // Recount the top level.
    let mut n = 0;
    let mut cur = root.dict_get("First")?;
    let mut guard = 0;
    while cur.kind()? == ObjectKind::Dictionary && guard < MAX_ITEMS {
        guard += 1;
        n += 1 + cur.dict_get("Count")?.as_int().unwrap_or(0).max(0);
        cur = cur.dict_get("Next")?;
    }
    root.dict_set("Count", &doc.new_int(n))?;
    Ok(())
}

/// Strip bookmarks that point at pages in `removed`
/// (explicit or named destination, or a `GoTo` action) so the removed page is not kept alive.
pub(crate) fn neutralise_for_deleted(
    cx: &mut EditContext<'_>,
    removed: &HashSet<ObjId>,
    names: &DestMap,
) -> Result<usize> {
    let doc = cx.doc();
    let forest = read_forest(doc)?;
    let mut stack: Vec<&Node> = forest.iter().collect();
    let mut count = 0;
    while let Some(n) = stack.pop() {
        stack.extend(n.kids.iter());
        if let Some((slot, t)) = dests::holder_dest(&n.obj)?
            && let Some(arr) = dests::resolve(names, &t)?
            && let Some(pid) = dests::dest_page(&arr)?
            && removed.contains(&pid)
        {
            cx.touch(&n.obj)?;
            dests::clear_dest(&n.obj, slot)?;
            count += 1;
        }
    }
    Ok(count)
}
