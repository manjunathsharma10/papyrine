use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::num;
use crate::changeset::ChangeSet;
use crate::command::Command;
use crate::context::EditContext;
use crate::error::{Error, Result};
use crate::text::LocalizedText;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoxKind {
    MediaBox,
    CropBox,
    BleedBox,
    TrimBox,
    ArtBox,
}

impl BoxKind {
    pub fn key(self) -> &'static str {
        match self {
            BoxKind::MediaBox => "MediaBox",
            BoxKind::CropBox => "CropBox",
            BoxKind::BleedBox => "BleedBox",
            BoxKind::TrimBox => "TrimBox",
            BoxKind::ArtBox => "ArtBox",
        }
    }
}

/// Set a page boundary box to `[llx, lly, urx, ury]` (corners may be given in any order), or
/// remove it with `rect: None` (not allowed for the media box).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetPageBox {
    pub page: usize,
    pub kind: BoxKind,
    pub rect: Option<[f64; 4]>,
}

impl SetPageBox {
    pub const NAME: &'static str = "set_page_box";

    pub fn new(page: usize, kind: BoxKind, rect: Option<[f64; 4]>) -> Self {
        SetPageBox { page, kind, rect }
    }
}

impl Command for SetPageBox {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.set-page-box")
            .arg("box", self.kind.key())
            .arg("page", self.page + 1)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        let n = doc.page_count()?;
        if self.page >= n {
            return Err(Error::invalid(format!(
                "page {} out of range (document has {n})",
                self.page
            )));
        }
        let page = doc.page(self.page)?;
        match self.rect {
            None if self.kind == BoxKind::MediaBox => {
                return Err(Error::invalid("the media box cannot be removed"));
            }
            None => cx.remove_key(&page, self.kind.key())?,
            Some(r) => {
                if r.iter().any(|v| !v.is_finite() || v.abs() > 1.0e7) {
                    return Err(Error::invalid(
                        "box coordinates must be finite and below 1e7",
                    ));
                }
                let (x0, x1) = (r[0].min(r[2]), r[0].max(r[2]));
                let (y0, y1) = (r[1].min(r[3]), r[1].max(r[3]));
                if x1 - x0 < 1.0 || y1 - y0 < 1.0 {
                    return Err(Error::invalid("box must be at least 1 point wide and high"));
                }
                let arr =
                    doc.parse_object(format!("[{} {} {} {}]", num(x0), num(y0), num(x1), num(y1)))?;
                cx.set_key(&page, self.kind.key(), &arr)?;
            }
        }
        cx.note_page(self.page);
        cx.changeset()
    }
}
