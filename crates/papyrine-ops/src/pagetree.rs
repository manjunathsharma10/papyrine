//! Page-tree helpers.
//!
//! qpdf flattens `/Pages` on the first `add_page`/`remove_page` and caches the page list. A
//! command that changes the page structure must record everything that flattening can touch
//! ([`touch_page_tree`]), and restoring the tree behind qpdf's back needs the cache rebuilt
//! ([`invalidate_page_cache`]).

use std::collections::HashSet;

use papyrine_cos::{Document, ObjId, Object};

use crate::context::EditContext;
use crate::error::Result;

const INHERITABLE: [&str; 4] = ["MediaBox", "CropBox", "Rotate", "Resources"];
const MAX_NODES: usize = 4_000_000;

fn is_node(o: &Object) -> Result<bool> {
    Ok(o.dict_has("Kids")?)
}

/// Record every object a page-tree edit can change. For the common flat tree that is just the
/// root `/Pages` node; for nested trees (or inherited attributes that flattening pushes down)
/// it is every node and page.
pub fn touch_page_tree(cx: &mut EditContext<'_>) -> Result<()> {
    let doc = cx.doc();
    let root = doc.root()?;
    let pages = root.dict_get("Pages")?;
    if pages.is_indirect() {
        cx.touch(&pages)?;
    } else {
        cx.touch(&root)?;
    }
    let kids = pages.dict_get("Kids")?;
    let mut flat = !INHERITABLE
        .iter()
        .any(|k| pages.dict_has(k).unwrap_or(true));
    if flat {
        for kid in kids.array_items()? {
            if !kid.is_indirect() || is_node(&kid)? || kid.dict_get("Parent")?.id() != pages.id() {
                flat = false;
                break;
            }
        }
    }
    if !flat {
        let mut seen: HashSet<ObjId> = HashSet::new();
        let mut stack = vec![pages];
        while let Some(node) = stack.pop() {
            if seen.len() > MAX_NODES {
                break;
            }
            if let Some(id) = node.id() {
                if !seen.insert(id) {
                    continue;
                }
                cx.touch(&node)?;
            }
            if is_node(&node)? {
                stack.extend(node.dict_get("Kids")?.array_items()?);
            }
        }
    }
    cx.mark_page_tree();
    Ok(())
}

/// Make qpdf rebuild its page cache from the raw tree on next use.
///
/// papyrine-cos does not expose `QPDF::updateAllPagesCache`, but qpdf recomputes the cache
/// whenever it is empty, and removing every page through the API empties it. This only edits
/// the root `/Kids` and `/Count`, which the caller restores right after from the change set's
/// images. Must run while the cache still matches the tree (i.e. before raw restores).
pub fn invalidate_page_cache(doc: &Document) -> Result<()> {
    let n = doc.page_count()?;
    for i in (0..n).rev() {
        let p = doc.page(i)?;
        doc.remove_page(&p)?;
    }
    Ok(())
}
