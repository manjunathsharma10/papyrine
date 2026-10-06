use papyrine_cos::ObjectKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::forms::{self, Cloner, FieldCloner};
use super::{insert_page_at, labels, normalize_pages};
use crate::changeset::ChangeSet;
use crate::command::Command;
use crate::context::EditContext;
use crate::error::Result;
use crate::image::deep_copy_direct;
use crate::text::LocalizedText;

/// Copy the selected pages; the copies form a block right after the last selected page.
///
/// A copy shares content streams, fonts and other resources with its original (they are
/// immutable once written) but gets its own annotation objects so editing an annotation on one
/// page cannot change the other. `/Popup` links are not duplicated. Form widgets become new
/// fields (renamed with a `_2`, `_3`, ... suffix) so the copy does not share a value with the
/// original. Page labels after the copies shift to make room.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicatePages {
    pub pages: Vec<usize>,
}

impl DuplicatePages {
    pub const NAME: &'static str = "duplicate_pages";

    pub fn new(pages: Vec<usize>) -> Self {
        DuplicatePages { pages }
    }
}

impl Command for DuplicatePages {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.duplicate-pages").arg("count", self.pages.len())
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        let pages = normalize_pages(doc, &self.pages)?;
        let originals = pages
            .iter()
            .map(|&i| doc.page(i))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let base = pages[pages.len() - 1] + 1;
        cx.touch_page_tree()?;
        labels::shift_for_insert(cx, base, originals.len())?;
        let mut fields = FieldCloner::new(Cloner {
            dest: doc,
            src: doc,
            foreign: false,
        });
        for (j, orig) in originals.iter().enumerate() {
            // qpdf copies a page that is already in the tree instead of sharing the object.
            insert_page_at(doc, orig, base + j)?;
            let copy = doc.page(base + j)?;
            let annots = copy.dict_get("Annots")?;
            if annots.kind()? == ObjectKind::Array {
                let fresh = doc.new_array();
                for a in annots.array_items()? {
                    if a.kind()? != ObjectKind::Dictionary
                        || a.dict_get("Subtype")?.name().is_ok_and(|n| n == b"Popup")
                    {
                        continue;
                    }
                    if forms::is_widget(&a)? {
                        fresh.array_push(&fields.widget(&a, &copy)?)?;
                        continue;
                    }
                    let c = deep_copy_direct(doc, &a)?;
                    c.dict_remove("Popup")?;
                    c.dict_set("P", &copy)?;
                    fresh.array_push(&doc.make_indirect(&c)?)?;
                }
                copy.dict_set("Annots", &fresh)?;
            }
        }
        forms::register_fields(cx, &fields.tops, doc, &fields.cloner)?;
        cx.note_structure();
        cx.changeset()
    }
}
