use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{insert_page_at, normalize_pages};
use crate::changeset::ChangeSet;
use crate::command::Command;
use crate::context::EditContext;
use crate::error::{Error, Result};
use crate::text::LocalizedText;

/// Move the selected pages (keeping their relative order) so they form a block that starts at
/// index `to` of the document *without* them (`0..=page_count - selected`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovePages {
    pub pages: Vec<usize>,
    pub to: usize,
}

impl MovePages {
    pub const NAME: &'static str = "move_pages";

    pub fn new(pages: Vec<usize>, to: usize) -> Self {
        MovePages { pages, to }
    }
}

impl Command for MovePages {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.move-pages")
            .arg("count", self.pages.len())
            .arg("to", self.to + 1)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        let pages = normalize_pages(doc, &self.pages)?;
        let remaining = doc.page_count()? - pages.len();
        if self.to > remaining {
            return Err(Error::invalid(format!(
                "destination {} out of range (0..={remaining})",
                self.to
            )));
        }
        let handles = pages
            .iter()
            .map(|&i| doc.page(i))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        cx.touch_page_tree()?;
        for h in &handles {
            cx.touch(h)?;
        }
        for h in handles.iter().rev() {
            doc.remove_page(h)?;
        }
        for (j, h) in handles.iter().enumerate() {
            insert_page_at(doc, h, self.to + j)?;
        }
        cx.note_structure();
        cx.changeset()
    }
}
