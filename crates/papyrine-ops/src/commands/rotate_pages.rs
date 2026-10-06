use papyrine_cos::Object;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::normalize_pages;
use super::selection::PageSelection;
use crate::changeset::ChangeSet;
use crate::command::Command;
use crate::context::EditContext;
use crate::error::{Error, Result};
use crate::text::LocalizedText;

/// Rotate pages clockwise by `delta` degrees (a multiple of 90, may be negative).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotatePages {
    pub pages: Vec<usize>,
    pub delta: i32,
}

impl RotatePages {
    pub const NAME: &'static str = "rotate_pages";

    pub fn new(pages: Vec<usize>, delta: i32) -> Self {
        RotatePages { pages, delta }
    }

    /// Rotate the pages a [`PageSelection`] (a range, odd or even pages, ...) picks in a
    /// document of `page_count` pages.
    pub fn selected(sel: &PageSelection, page_count: usize, delta: i32) -> Result<Self> {
        Ok(Self::new(sel.resolve(page_count)?, delta))
    }
}

/// `/Rotate` inherited from the page's ancestors (not the page itself).
fn inherited_rotate(page: &Object) -> Result<Option<i64>> {
    let mut cur = page.dict_get("Parent")?;
    for _ in 0..64 {
        if cur.is_null()? {
            break;
        }
        if cur.dict_has("Rotate")? {
            return Ok(Some(cur.dict_get("Rotate")?.as_f64()?.round() as i64));
        }
        cur = cur.dict_get("Parent")?;
    }
    Ok(None)
}

fn effective_rotate(page: &Object) -> Result<i64> {
    if page.dict_has("Rotate")? {
        return Ok(page.dict_get("Rotate")?.as_f64()?.round() as i64);
    }
    Ok(inherited_rotate(page)?.unwrap_or(0))
}

impl Command for RotatePages {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.rotate-pages")
            .arg("count", self.pages.len())
            .arg("degrees", self.delta)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        if self.delta % 90 != 0 {
            return Err(Error::invalid("rotation must be a multiple of 90 degrees"));
        }
        let pages = normalize_pages(doc, &self.pages)?;
        if self.delta == 0 {
            return cx.changeset();
        }
        for idx in pages {
            let page = doc.page(idx)?;
            let new = (effective_rotate(&page)? + i64::from(self.delta)).rem_euclid(360);
            cx.touch(&page)?;
            if new == 0 && inherited_rotate(&page)?.is_none() {
                page.dict_remove("Rotate")?;
            } else {
                page.dict_set("Rotate", &doc.new_int(new))?;
            }
            cx.note_page(idx);
        }
        cx.changeset()
    }
}
