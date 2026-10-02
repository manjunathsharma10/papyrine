//! Page tree: ordered page list, inherited attributes, boxes and rotation.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use papyrine_core::geom::{Rect, Size};
use papyrine_cos::{Error, ObjId, Object, Result};

use crate::dest::Action;
use crate::model::{Key, MAX_DEPTH, Model, PAGE_TREE};

/// US Letter, used when a page has no usable MediaBox (as Acrobat and PDFium do).
pub const DEFAULT_MEDIA_BOX: Rect = Rect::new(0.0, 0.0, 612.0, 792.0);

/// Ordered ids of every page, plus the reverse lookup.
#[derive(Debug)]
pub struct PageList {
    ids: Vec<ObjId>,
    index: HashMap<ObjId, usize>,
}

impl PageList {
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn ids(&self) -> &[ObjId] {
        &self.ids
    }
    pub fn id(&self, index: usize) -> Option<ObjId> {
        self.ids.get(index).copied()
    }
    pub fn index_of(&self, id: ObjId) -> Option<usize> {
        self.index.get(&id).copied()
    }
}

/// The five page boxes, each already resolved to its effective value, plus what the page
/// itself declared (inheritance applied for MediaBox and CropBox only).
#[derive(Debug, Clone, PartialEq)]
pub struct PageBoxes {
    pub media: Rect,
    /// CropBox clipped to MediaBox; MediaBox when absent.
    pub crop: Rect,
    /// BleedBox, TrimBox and ArtBox default to the effective CropBox and are clipped to the MediaBox.
    pub bleed: Rect,
    pub trim: Rect,
    pub art: Rect,
    pub declared_media: Option<Rect>,
    pub declared_crop: Option<Rect>,
    pub declared_bleed: Option<Rect>,
    pub declared_trim: Option<Rect>,
    pub declared_art: Option<Rect>,
}

#[derive(Debug, Clone)]
pub struct PageAction {
    /// `"O"` (page opened) or `"C"` (page closed).
    pub trigger: String,
    pub action: Action,
}

#[derive(Debug, Clone)]
pub struct PageInfo {
    pub index: usize,
    pub id: ObjId,
    pub boxes: PageBoxes,
    /// Clockwise display rotation in degrees: 0, 90, 180 or 270.
    pub rotate: u16,
    pub user_unit: f64,
    pub resources: Option<Object>,
    pub has_contents: bool,
    pub annotation_count: usize,
    /// `/Tabs`: `R`, `C`, `S` (tab order), `W`, if present.
    pub tab_order: Option<String>,
    pub actions: Vec<PageAction>,
}

impl PageInfo {
    /// Displayed size in points: the crop box, rotated, scaled by UserUnit.
    pub fn display_size(&self) -> Size {
        let c = self.boxes.crop;
        let (w, h) = (c.width() * self.user_unit, c.height() * self.user_unit);
        if self.rotate % 180 == 90 {
            Size::new(h, w)
        } else {
            Size::new(w, h)
        }
    }
}

impl Model {
    /// Ordered page list (cached; invalidated when page-tree nodes change).
    pub fn page_list(&self) -> Result<Rc<PageList>> {
        self.cached(Key::PageList, |m| {
            let pages = m.document().pages()?;
            let mut ids = Vec::with_capacity(pages.len());
            // Only internal nodes: an edit to a leaf page does not change the list.
            let mut nodes: HashSet<ObjId> = HashSet::new();
            for p in &pages {
                ids.push(p.id().unwrap_or(ObjId::new(0, 0)));
                let mut cur = m.get(p, "Parent");
                for _ in 0..MAX_DEPTH {
                    let Some(node) = cur else { break };
                    if !m.is_dict(&node) {
                        break;
                    }
                    if let Some(i) = node.id()
                        && !nodes.insert(i)
                    {
                        break;
                    }
                    cur = m.get(&node, "Parent");
                }
            }
            m.register_tree_nodes(nodes);
            m.set_frame_deps(vec![PAGE_TREE]);
            let index = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
            Ok(PageList { ids, index })
        })
    }

    pub fn page_count(&self) -> Result<usize> {
        Ok(self.page_list()?.len())
    }

    /// The page dictionary of page `index`.
    pub fn page_object(&self, index: usize) -> Result<Object> {
        let list = self.page_list()?;
        if index >= list.len() {
            return Err(Error::Range(format!(
                "page {index} out of range (0..{})",
                list.len()
            )));
        }
        self.document().page(index)
    }

    /// Index of the page with this object id.
    pub fn page_index_of(&self, id: ObjId) -> Result<Option<usize>> {
        Ok(self.page_list()?.index_of(id))
    }

    /// Typed view of one page, materialized on first use.
    pub fn page(&self, index: usize) -> Result<Rc<PageInfo>> {
        self.cached(Key::Page(index), |m| {
            m.dep_id(PAGE_TREE);
            let page = m.page_object(index)?;
            Ok(m.build_page(index, &page))
        })
    }

    fn build_page(&self, index: usize, page: &Object) -> PageInfo {
        let id = page.id().unwrap_or(ObjId::new(0, 0));
        let rect_of = |key: &str, inherit: bool| -> Option<Rect> {
            let v = if inherit {
                self.inherited(page, key)
            } else {
                self.get(page, key)
            }?;
            let r = self.rect(&v);
            if r.is_none() {
                self.note(format!("page {index}: malformed /{key}"));
            }
            r.filter(|r| r.width() > 0.0 && r.height() > 0.0)
        };
        let declared_media = rect_of("MediaBox", true);
        let declared_crop = rect_of("CropBox", true);
        let declared_bleed = rect_of("BleedBox", false);
        let declared_trim = rect_of("TrimBox", false);
        let declared_art = rect_of("ArtBox", false);
        let media = declared_media.unwrap_or(DEFAULT_MEDIA_BOX);
        let clip = |r: Rect| r.intersect(&media).unwrap_or(media);
        let crop = declared_crop.map_or(media, clip);
        let boxed = |d: Option<Rect>| d.map_or(crop, clip);
        let boxes = PageBoxes {
            media,
            crop,
            bleed: boxed(declared_bleed),
            trim: boxed(declared_trim),
            art: boxed(declared_art),
            declared_media,
            declared_crop,
            declared_bleed,
            declared_trim,
            declared_art,
        };
        let rotate = self
            .inherited(page, "Rotate")
            .and_then(|v| self.num(&v))
            .map_or(0, normalize_rotation);
        let user_unit = self
            .get(page, "UserUnit")
            .and_then(|v| self.num(&v))
            .filter(|u| *u > 0.0)
            .unwrap_or(1.0);
        let contents = self.get(page, "Contents");
        let has_contents = contents.is_some_and(|c| match self.kind(&c) {
            papyrine_cos::ObjectKind::Array => !self.items(&c).is_empty(),
            papyrine_cos::ObjectKind::Stream => true,
            _ => false,
        });
        let annotation_count = self.items_of(page, "Annots").len();
        let mut actions = Vec::new();
        if let Some(aa) = self.get(page, "AA") {
            for trigger in ["O", "C"] {
                if let Some(a) = self.get(&aa, trigger).and_then(|a| self.parse_action(&a)) {
                    actions.push(PageAction {
                        trigger: trigger.into(),
                        action: a,
                    });
                }
            }
        }
        PageInfo {
            index,
            id,
            boxes,
            rotate,
            user_unit,
            resources: self.inherited(page, "Resources"),
            has_contents,
            annotation_count,
            tab_order: self.name_of(page, "Tabs"),
            actions,
        }
    }
}

/// Round to a multiple of 90 in `0..360`, accepting negatives and values over 360.
pub fn normalize_rotation(degrees: f64) -> u16 {
    let d = (degrees / 90.0).trunc() as i64 * 90;
    d.rem_euclid(360) as u16
}

#[cfg(test)]
mod tests {
    use super::normalize_rotation;

    #[test]
    fn rotation() {
        assert_eq!(normalize_rotation(0.0), 0);
        assert_eq!(normalize_rotation(90.0), 90);
        assert_eq!(normalize_rotation(-90.0), 270);
        assert_eq!(normalize_rotation(450.0), 90);
        assert_eq!(normalize_rotation(-360.0), 0);
        assert_eq!(normalize_rotation(100.0), 90);
    }
}
