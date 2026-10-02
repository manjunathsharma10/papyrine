use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{insert_page_at, num};
use crate::changeset::ChangeSet;
use crate::command::Command;
use crate::context::EditContext;
use crate::error::{Error, Result};
use crate::text::LocalizedText;

/// Insert an empty page of `width` x `height` points so that it becomes page `at`
/// (`0..=page_count`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertBlankPage {
    pub at: usize,
    pub width: f64,
    pub height: f64,
}

impl InsertBlankPage {
    pub const NAME: &'static str = "insert_blank_page";

    pub fn new(at: usize, width: f64, height: f64) -> Self {
        InsertBlankPage { at, width, height }
    }

    /// US Letter.
    pub fn letter(at: usize) -> Self {
        Self::new(at, 612.0, 792.0)
    }
}

impl Command for InsertBlankPage {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.insert-blank-page").arg("at", self.at + 1)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        let ok = |v: f64| v.is_finite() && (1.0..=14400.0).contains(&v);
        if !ok(self.width) || !ok(self.height) {
            return Err(Error::invalid(
                "page size must be between 1 and 14400 points",
            ));
        }
        let n = doc.page_count()?;
        if self.at > n {
            return Err(Error::invalid(format!(
                "position {} out of range (0..={n})",
                self.at
            )));
        }
        cx.touch_page_tree()?;
        let page = doc.parse_object(format!(
            "<< /Type /Page /MediaBox [0 0 {} {}] /Resources << >> >>",
            num(self.width),
            num(self.height)
        ))?;
        page.dict_set("Contents", &doc.new_stream(b"")?)?;
        insert_page_at(doc, &page, self.at)?;
        cx.note_structure();
        cx.changeset()
    }
}
