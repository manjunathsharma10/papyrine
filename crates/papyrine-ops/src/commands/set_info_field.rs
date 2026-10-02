use papyrine_cos::ObjectKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::changeset::ChangeSet;
use crate::command::Command;
use crate::context::EditContext;
use crate::error::Result;
use crate::text::{LocalizedText, encode_text_string};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InfoField {
    Title,
    Author,
    Subject,
    Keywords,
}

impl InfoField {
    pub fn key(self) -> &'static str {
        match self {
            InfoField::Title => "Title",
            InfoField::Author => "Author",
            InfoField::Subject => "Subject",
            InfoField::Keywords => "Keywords",
        }
    }
}

/// Set (or, with `value: None`, remove) a document-information field. Creates the `/Info`
/// dictionary when the file has none. XMP metadata is not synchronised here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetInfoField {
    pub field: InfoField,
    pub value: Option<String>,
}

impl SetInfoField {
    pub const NAME: &'static str = "set_info_field";

    pub fn new(field: InfoField, value: Option<String>) -> Self {
        SetInfoField { field, value }
    }
}

impl Command for SetInfoField {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.set-info-field").arg("field", self.field.key())
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let doc = cx.doc();
        let trailer = doc.trailer()?;
        let mut info = trailer.dict_get("Info")?;
        if info.kind()? != ObjectKind::Dictionary {
            if self.value.is_none() {
                return cx.changeset();
            }
            cx.touch_trailer()?;
            info = doc.make_indirect(&doc.new_dict())?;
            trailer.dict_set("Info", &info)?;
        } else if info.is_indirect() {
            cx.touch(&info)?;
        } else {
            cx.touch_trailer()?;
        }
        match &self.value {
            Some(v) => info.dict_set(self.field.key(), &doc.new_string(encode_text_string(v))?)?,
            None => info.dict_remove(self.field.key())?,
        }
        cx.changeset()
    }
}
