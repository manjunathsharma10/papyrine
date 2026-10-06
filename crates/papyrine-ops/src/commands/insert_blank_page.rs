use papyrine_cos::{Object, ObjectKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{insert_page_at, labels, num};
use crate::changeset::ChangeSet;
use crate::command::Command;
use crate::context::EditContext;
use crate::error::{Error, Result};
use crate::text::LocalizedText;

/// Standard paper sizes (portrait, in points).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageSize {
    Letter,
    Legal,
    Tabloid,
    A3,
    A4,
    A5,
    B5,
}

impl PageSize {
    pub fn points(self) -> (f64, f64) {
        match self {
            PageSize::Letter => (612.0, 792.0),
            PageSize::Legal => (612.0, 1008.0),
            PageSize::Tabloid => (792.0, 1224.0),
            PageSize::A3 => (841.89, 1190.55),
            PageSize::A4 => (595.28, 841.89),
            PageSize::A5 => (419.53, 595.28),
            PageSize::B5 => (498.9, 708.66),
        }
    }
}

/// Insert an empty page of `width` x `height` points so that it becomes page `at`
/// (`0..=page_count`). With `like`, the size is that of the page with this index (as it shows:
/// crop box, rotation applied) and `width`/`height` are ignored. Page labels after the new page
/// shift to make room; the new page continues the label sequence of the page before it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertBlankPage {
    pub at: usize,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub like: Option<usize>,
}

/// A page box (`CropBox`, else `MediaBox`), inherited from ancestors, normalised, with the
/// rotation applied: the size the page shows at.
fn shown_size(page: &Object) -> Result<(f64, f64)> {
    let mut rotate = 0i64;
    let mut have_rotate = false;
    let mut media: Option<Object> = None;
    let mut crop: Option<Object> = None;
    let mut cur = page.clone();
    for _ in 0..64 {
        if cur.kind()? != ObjectKind::Dictionary {
            break;
        }
        if !have_rotate && cur.dict_has("Rotate")? {
            rotate = cur.dict_get("Rotate")?.as_f64().unwrap_or(0.0).round() as i64;
            have_rotate = true;
        }
        if crop.is_none() && cur.dict_has("CropBox")? {
            crop = Some(cur.dict_get("CropBox")?);
        }
        if media.is_none() && cur.dict_has("MediaBox")? {
            media = Some(cur.dict_get("MediaBox")?);
        }
        cur = cur.dict_get("Parent")?;
    }
    let b = crop
        .or(media)
        .ok_or_else(|| Error::invalid("reference page has no page box"))?;
    if b.kind()? != ObjectKind::Array || b.array_len()? != 4 {
        return Err(Error::invalid("reference page box is malformed"));
    }
    let v = (0..4)
        .map(|i| Ok(b.array_get(i)?.as_f64()?))
        .collect::<Result<Vec<f64>>>()?;
    let (w, h) = ((v[2] - v[0]).abs(), (v[3] - v[1]).abs());
    Ok(if rotate.rem_euclid(180) == 90 {
        (h, w)
    } else {
        (w, h)
    })
}

impl InsertBlankPage {
    pub const NAME: &'static str = "insert_blank_page";

    pub fn new(at: usize, width: f64, height: f64) -> Self {
        InsertBlankPage {
            at,
            width,
            height,
            like: None,
        }
    }

    /// A standard size, optionally landscape.
    pub fn preset(at: usize, size: PageSize, landscape: bool) -> Self {
        let (w, h) = size.points();
        if landscape {
            Self::new(at, h, w)
        } else {
            Self::new(at, w, h)
        }
    }

    /// Same size as the page that is currently page `reference` (zero-based).
    pub fn like_page(at: usize, reference: usize) -> Self {
        InsertBlankPage {
            like: Some(reference),
            ..Self::new(at, 612.0, 792.0)
        }
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
        let (width, height) = match self.like {
            Some(r) => shown_size(&doc.page(r)?)?,
            None => (self.width, self.height),
        };
        let ok = |v: f64| v.is_finite() && (1.0..=14400.0).contains(&v);
        if !ok(width) || !ok(height) {
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
        labels::shift_for_insert(cx, self.at, 1)?;
        let page = doc.parse_object(format!(
            "<< /Type /Page /MediaBox [0 0 {} {}] /Resources << >> >>",
            num(width),
            num(height)
        ))?;
        page.dict_set("Contents", &doc.new_stream(b"")?)?;
        insert_page_at(doc, &page, self.at)?;
        cx.note_structure();
        cx.changeset()
    }
}
