//! Importing pages from another PDF: the machinery behind "insert pages from a file", merge,
//! extract and split.
//!
//! qpdf's own page copy drags a form's whole field tree (and all its widgets) along with every
//! page and renames colliding fields by splitting hierarchies, so pages are copied here one at
//! a time as a thin clone of the page dictionary: content, resources and non-widget
//! annotations go through qpdf's foreign-object copier (which shares fonts and images between
//! pages of the same source); widgets are rebuilt together with only the ancestors they need.
//! Links, bookmarks, named destinations and fields are then pointed at the copies.

use std::collections::{HashMap, HashSet};

use papyrine_cos::{Document, ObjId, Object, ObjectKind};

use crate::commands::dests::{self, DestMap, Target};
use crate::commands::forms::{self, Cloner, FieldCloner, FieldReport};
use crate::commands::outlines::{self, OutlineMode, Place, Rebuild};
use crate::commands::{insert_page_at, labels};
use crate::context::EditContext;
use crate::error::{Error, Result};
use crate::image::deep_copy_direct;
use crate::text::encode_text_string;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    pub outline: OutlineMode,
    /// Title of the top-level bookmark in [`OutlineMode::PerFile`] (usually the file name).
    pub outline_title: String,
    /// Copy the named destinations that lead to imported pages (renamed on collision).
    pub named_dests: bool,
    /// Put bookmarks next to the destination bookmarks that lead to later pages instead of at
    /// the end. For inserting into an existing document.
    pub place_by_page: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions {
            outline: OutlineMode::Merged,
            outline_title: String::new(),
            named_dests: true,
            place_by_page: false,
        }
    }
}

/// What an import did.
#[derive(Debug, Default)]
pub struct Imported {
    /// The new page objects, in the order of the requested indices.
    pub pages: Vec<Object>,
    pub fields: FieldReport,
    /// Links (and actions) whose target page was not imported.
    pub links_dropped: usize,
    /// Bookmarks created in the destination.
    pub bookmarks: usize,
    pub dests_copied: usize,
    /// `(old, new)` for named destinations renamed to avoid a collision.
    pub dests_renamed: Vec<(String, String)>,
}

/// Where an annotation of the source page went.
enum Annot {
    Widget(Object),
    Plain { src: Object, copy: Object },
}

struct Copied {
    page: Object,
    annots: Vec<Annot>,
}

fn copy_page(
    dest: &Document,
    src: &Document,
    src_page: &Object,
    fields: &mut FieldCloner<'_>,
) -> Result<Copied> {
    let annots = src_page.dict_get("Annots")?;
    let items = if annots.kind()? == ObjectKind::Array {
        annots.array_items()?
    } else {
        vec![]
    };
    let mut plain: Vec<Object> = vec![];
    for a in &items {
        if a.kind()? == ObjectKind::Dictionary && !forms::is_widget(a)? {
            plain.push(a.clone());
        }
    }
    let tp = deep_copy_direct(src, src_page)?;
    tp.dict_remove("Parent")?;
    tp.dict_remove("Annots")?;
    if !plain.is_empty() {
        let arr = src.new_array();
        for a in &plain {
            arr.array_push(a)?;
        }
        tp.dict_set("Annots", &arr)?;
    }
    let tp = src.make_indirect(&tp)?;
    let np = dest.copy_foreign(&tp)?;
    let copied_plain_vec = if plain.is_empty() {
        vec![]
    } else {
        np.dict_get("Annots")?.array_items()?
    };
    let mut copied_plain = copied_plain_vec.into_iter();
    let mut slots = vec![];
    for a in items {
        if a.kind()? != ObjectKind::Dictionary {
            continue;
        }
        if forms::is_widget(&a)? {
            slots.push(Annot::Widget(fields.widget(&a, &np)?));
        } else {
            let copy = copied_plain
                .next()
                .ok_or_else(|| Error::Corrupt("annotation copy lost".into()))?;
            if copy.kind()? == ObjectKind::Dictionary && copy.dict_has("P")? {
                copy.dict_set("P", &np)?;
            }
            slots.push(Annot::Plain { src: a, copy });
        }
    }
    Ok(Copied {
        page: np,
        annots: slots,
    })
}

/// Rewrite the destinations of the copied annotations and rebuild the page's `/Annots`.
/// Returns how many links were dropped.
fn finish_annots(
    doc: &Document,
    c: &Copied,
    page_map: &HashMap<ObjId, Object>,
    names_map: &HashMap<Vec<u8>, Vec<u8>>,
) -> Result<usize> {
    let mut dropped = 0;
    let out = doc.new_array();
    for s in &c.annots {
        match s {
            Annot::Widget(w) => out.array_push(w)?,
            Annot::Plain { src, copy } => {
                let Some((slot, target)) = dests::holder_dest(src)? else {
                    out.array_push(copy)?;
                    continue;
                };
                let new = match &target {
                    Target::Explicit(arr) => {
                        match dests::dest_page(arr)?.and_then(|p| page_map.get(&p)) {
                            Some(np) => Some(dests::retarget(doc, arr, np)?),
                            None => None,
                        }
                    }
                    // A name we did not copy leads to a page that was not imported (or nowhere).
                    Target::Named(n) => match names_map.get(n) {
                        Some(nn) => Some(doc.new_string(nn)?),
                        None => None,
                    },
                };
                match new {
                    Some(v) => {
                        dests::set_dest(copy, slot, &v)?;
                        out.array_push(copy)?;
                    }
                    None => {
                        dropped += 1;
                        if copy.dict_get("Subtype")?.name().is_ok_and(|n| n == b"Link") {
                            continue;
                        }
                        dests::clear_dest(copy, slot)?;
                        out.array_push(copy)?;
                    }
                }
            }
        }
    }
    if out.array_len()? == 0 {
        c.page.dict_remove("Annots")?;
    } else {
        c.page.dict_set("Annots", &out)?;
    }
    Ok(dropped)
}

/// Copy the source's named destinations that lead to imported pages. Returns old -> new names.
fn copy_named_dests(
    cx: &mut EditContext<'_>,
    src_names: &DestMap,
    page_map: &HashMap<ObjId, Object>,
    rep: &mut Imported,
) -> Result<HashMap<Vec<u8>, Vec<u8>>> {
    let mut map = HashMap::new();
    let mut todo: Vec<(&Vec<u8>, Object, bool)> = vec![];
    for (name, value) in &src_names.entries {
        let Some(arr) = dests::dest_array(value)? else {
            continue;
        };
        let Some(np) = dests::dest_page(&arr)?.and_then(|p| page_map.get(&p)) else {
            continue;
        };
        let is_dict = value.kind()? == ObjectKind::Dictionary;
        todo.push((name, dests::retarget(cx.doc(), &arr, np)?, is_dict));
    }
    if todo.is_empty() {
        return Ok(map);
    }
    dests::touch_dest_tree(cx)?;
    let doc = cx.doc();
    let mut taken = DestMap::collect(doc)?.names();
    let tree = doc
        .catalog_name_tree("Dests", true)?
        .ok_or_else(|| Error::Corrupt("cannot create /Dests name tree".into()))?;
    for (name, arr, is_dict) in todo {
        let fresh = dests::unique_name(&taken, name);
        taken.insert(fresh.clone());
        let value = if is_dict {
            let d = doc.new_dict();
            d.dict_set("D", &arr)?;
            d
        } else {
            arr
        };
        doc.name_tree_set(&tree, &fresh, &value)?;
        if &fresh != name {
            rep.dests_renamed.push((
                String::from_utf8_lossy(name).into_owned(),
                String::from_utf8_lossy(&fresh).into_owned(),
            ));
        }
        map.insert(name.clone(), fresh);
        rep.dests_copied += 1;
    }
    Ok(map)
}

/// qpdf adds `/Length` to a lazily copied stream the first time its data is read, which would
/// make the object look different between two images of the same state. Read every new stream
/// once now so the recorded images stay stable.
fn materialize_streams(doc: &Document, base_max: u32) -> Result<()> {
    for id in doc.object_ids()? {
        if id.num > base_max {
            let o = doc.object(id)?;
            if o.kind()? == ObjectKind::Stream {
                o.stream_raw()?;
            }
        }
    }
    Ok(())
}

/// Copy pages `indices` of `src` into the destination so the first becomes page `at`.
///
/// Everything the command mutates in the destination is recorded in `cx` first, so the whole
/// import is one undoable step. `src` is modified (inherited page attributes are pushed down
/// to the pages) and scratch objects are added to it; pass a document opened for this purpose.
pub fn import_pages(
    cx: &mut EditContext<'_>,
    src: &Document,
    indices: &[usize],
    at: usize,
    opts: &ImportOptions,
) -> Result<Imported> {
    let doc = cx.doc();
    if indices.is_empty() {
        return Err(Error::invalid("no pages to import"));
    }
    let n_src = src.page_count()?;
    if let Some(&bad) = indices.iter().find(|&&i| i >= n_src) {
        return Err(Error::invalid(format!(
            "source page {bad} out of range (source has {n_src})"
        )));
    }
    let n_dest = doc.page_count()?;
    if at > n_dest {
        return Err(Error::invalid(format!(
            "position {at} out of range (0..={n_dest})"
        )));
    }
    src.push_inherited_page_attributes()?;
    cx.touch_page_tree()?;
    let base_max = doc.object_ids()?.iter().map(|i| i.num).max().unwrap_or(0);

    let mut rep = Imported::default();
    let cloner = Cloner {
        dest: doc,
        src,
        foreign: true,
    };
    let mut fields = FieldCloner::new(Cloner {
        dest: doc,
        src,
        foreign: true,
    });
    let mut copies: Vec<Copied> = Vec::with_capacity(indices.len());
    let mut page_map: HashMap<ObjId, Object> = HashMap::new();
    for &i in indices {
        let sp = src.page(i)?;
        let c = copy_page(doc, src, &sp, &mut fields)?;
        if let Some(id) = sp.id() {
            page_map.entry(id).or_insert_with(|| c.page.clone());
        }
        copies.push(c);
    }
    labels::shift_for_insert(cx, at, indices.len())?;
    for (j, c) in copies.iter().enumerate() {
        insert_page_at(doc, &c.page, at + j)?;
    }

    let src_names = DestMap::collect(src)?;
    let names_map = if opts.named_dests {
        copy_named_dests(cx, &src_names, &page_map, &mut rep)?
    } else {
        HashMap::new()
    };
    for c in &copies {
        rep.links_dropped += finish_annots(doc, c, &page_map, &names_map)?;
    }
    rep.fields = forms::register_fields(cx, &fields.tops, src, &cloner)?;

    if opts.outline != OutlineMode::None {
        let forest = outlines::read_forest(src)?;
        let distinct: HashSet<usize> = indices.iter().copied().collect();
        let rb = Rebuild {
            dest: doc,
            src_names: &src_names,
            page_map: &page_map,
            full: distinct.len() == n_src,
            created: Default::default(),
        };
        let wanted = opts.outline == OutlineMode::PerFile || rb.any_kept(&forest)?;
        if wanted {
            let root = outlines::root_for_build(cx)?;
            let group: Vec<Object> = if opts.outline == OutlineMode::PerFile {
                let w = doc.make_indirect(&doc.new_dict())?;
                rb.created.set(rb.created.get() + 1);
                let title = if opts.outline_title.is_empty() {
                    "Document"
                } else {
                    &opts.outline_title
                };
                w.dict_set("Title", &doc.new_string(encode_text_string(title))?)?;
                w.dict_set("Parent", &root)?;
                w.dict_set("Dest", &dests::fit_dest(doc, &copies[0].page)?)?;
                let kids = rb.build(&forest, &w)?;
                if let (Some(f), Some(l)) = (kids.first(), kids.last()) {
                    w.dict_set("First", f)?;
                    w.dict_set("Last", l)?;
                    let visible: i64 = kids
                        .iter()
                        .map(|k| {
                            1 + k
                                .dict_get("Count")
                                .ok()
                                .and_then(|c| c.as_int().ok())
                                .unwrap_or(0)
                                .max(0)
                        })
                        .sum();
                    w.dict_set("Count", &doc.new_int(-visible))?;
                }
                vec![w]
            } else {
                rb.build(&forest, &root)?
            };
            let place = if opts.place_by_page {
                Place::BeforePage(at + indices.len())
            } else {
                Place::End
            };
            outlines::attach(cx, &root, &group, place)?;
            rep.bookmarks = rb.created.get();
        }
    }
    materialize_streams(doc, base_max)?;
    cx.note_structure();
    rep.pages = copies.into_iter().map(|c| c.page).collect();
    Ok(rep)
}
