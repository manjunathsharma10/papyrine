//! Keeping deleted pages from lingering: after pages leave the page tree, bookmarks, links,
//! named destinations, form widgets and the open action that point at them are removed or
//! made inert. Otherwise qpdf writes the deleted page (and its content) as a stray object.

use std::collections::HashSet;

use papyrine_cos::{ObjId, Object, ObjectKind};

use crate::commands::dests::{self, DestMap};
use crate::commands::{forms, outlines};
use crate::context::EditContext;
use crate::error::Result;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScrubReport {
    pub bookmarks: usize,
    pub links: usize,
    pub names: usize,
}

pub(crate) fn scrub_deleted_pages(
    cx: &mut EditContext<'_>,
    removed: &HashSet<ObjId>,
) -> Result<ScrubReport> {
    let doc = cx.doc();
    let mut rep = ScrubReport::default();
    let names = DestMap::collect(doc)?;
    rep.bookmarks = outlines::neutralise_for_deleted(cx, removed, &names)?;
    rep.links = scrub_links(cx, removed, &names)?;
    rep.names = scrub_names(cx, removed, &names)?;
    scrub_open_action(cx, removed, &names)?;
    forms::drop_widgets_of_pages(cx, removed)?;
    Ok(rep)
}

fn points_at_removed(
    holder: &Object,
    removed: &HashSet<ObjId>,
    names: &DestMap,
) -> Result<Option<dests::Slot>> {
    if let Some((slot, t)) = dests::holder_dest(holder)?
        && let Some(arr) = dests::resolve(names, &t)?
        && let Some(pid) = dests::dest_page(&arr)?
        && removed.contains(&pid)
    {
        return Ok(Some(slot));
    }
    Ok(None)
}

fn scrub_links(
    cx: &mut EditContext<'_>,
    removed: &HashSet<ObjId>,
    names: &DestMap,
) -> Result<usize> {
    let doc = cx.doc();
    let mut n = 0;
    for page in doc.pages()? {
        let annots = page.dict_get("Annots")?;
        if annots.kind()? != ObjectKind::Array {
            continue;
        }
        let items = annots.array_items()?;
        let mut doomed: Vec<usize> = vec![];
        for (i, a) in items.iter().enumerate() {
            if a.kind()? != ObjectKind::Dictionary {
                continue;
            }
            let Some(slot) = points_at_removed(a, removed, names)? else {
                continue;
            };
            n += 1;
            if a.dict_get("Subtype")?.name().is_ok_and(|s| s == b"Link") {
                doomed.push(i);
            } else {
                cx.touch(a)?;
                dests::clear_dest(a, slot)?;
            }
        }
        if !doomed.is_empty() {
            cx.touch(&page)?;
            if annots.is_indirect() {
                cx.touch(&annots)?;
            }
            for i in doomed.into_iter().rev() {
                annots.array_remove(i)?;
            }
        }
    }
    Ok(n)
}

fn scrub_names(
    cx: &mut EditContext<'_>,
    removed: &HashSet<ObjId>,
    names: &DestMap,
) -> Result<usize> {
    let doomed: Vec<Vec<u8>> = names
        .entries
        .iter()
        .filter_map(|(k, v)| {
            let arr = dests::dest_array(v).ok().flatten()?;
            let pid = dests::dest_page(&arr).ok().flatten()?;
            removed.contains(&pid).then(|| k.clone())
        })
        .collect();
    if doomed.is_empty() {
        return Ok(0);
    }
    dests::touch_dest_tree(cx)?;
    let doc = cx.doc();
    let root = doc.root()?;
    let legacy = root.dict_get("Dests")?;
    if legacy.kind()? == ObjectKind::Dictionary && legacy.is_indirect() {
        cx.touch(&legacy)?;
    }
    let tree = doc.catalog_name_tree("Dests", false)?;
    for k in &doomed {
        if let Some(t) = &tree {
            doc.name_tree_remove(t, k)?;
        }
        if legacy.kind()? == ObjectKind::Dictionary {
            legacy.dict_remove(k)?;
        }
    }
    Ok(doomed.len())
}

fn scrub_open_action(
    cx: &mut EditContext<'_>,
    removed: &HashSet<ObjId>,
    names: &DestMap,
) -> Result<()> {
    let doc = cx.doc();
    let root = doc.root()?;
    let oa = root.dict_get("OpenAction")?;
    let hit = match oa.kind()? {
        ObjectKind::Array => dests::dest_page(&oa)?.is_some_and(|p| removed.contains(&p)),
        ObjectKind::Dictionary => {
            let d = oa.dict_get("D")?;
            let arr = match d.kind()? {
                ObjectKind::Array => Some(d),
                ObjectKind::String => names.array(&d.string()?)?,
                ObjectKind::Name => names.array(&d.name()?)?,
                _ => None,
            };
            match arr {
                Some(a) => dests::dest_page(&a)?.is_some_and(|p| removed.contains(&p)),
                None => false,
            }
        }
        _ => false,
    };
    if hit {
        cx.touch(&root)?;
        root.dict_remove("OpenAction")?;
    }
    Ok(())
}
