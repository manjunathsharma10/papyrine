use serde::{Deserialize, Serialize};
use serde_json::Value;

use std::collections::HashSet;

use super::{labels, normalize_pages, scrub};
use crate::changeset::ChangeSet;
use crate::command::Command;
use crate::context::EditContext;
use crate::error::{Error, Result};
use crate::text::LocalizedText;

/// Remove pages from the page tree. The page objects stay in memory (so undo is exact) and are
/// dropped from the file on write as unreferenced. A document must keep at least one page.
///
/// Bookmarks, links, named destinations, form widgets and the open action that pointed at a
/// deleted page are removed or made inert (otherwise the page would stay in the file), and
/// page-label ranges after the deleted pages move up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletePages {
    pub pages: Vec<usize>,
}

impl DeletePages {
    pub const NAME: &'static str = "delete_pages";

    pub fn new(pages: Vec<usize>) -> Self {
        DeletePages { pages }
    }
}

impl Command for DeletePages {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.delete-pages").arg("count", self.pages.len())
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        let pages = normalize_pages(doc, &self.pages)?;
        if pages.len() >= doc.page_count()? {
            return Err(Error::invalid("cannot delete every page"));
        }
        let handles = pages
            .iter()
            .map(|&i| doc.page(i))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let removed: HashSet<_> = handles.iter().filter_map(|h| h.id()).collect();
        let old_count = doc.page_count()?;
        cx.touch_page_tree()?;
        for h in &handles {
            doc.remove_page(h)?;
        }
        labels::shift_for_delete(cx, &pages, old_count)?;
        scrub::scrub_deleted_pages(cx, &removed)?;
        cx.note_structure();
        cx.changeset()
    }
}
