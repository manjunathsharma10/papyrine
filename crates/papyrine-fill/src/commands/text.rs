use papyrine_ops::{ChangeSet, Command, EditContext, LocalizedText, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::engine;
use crate::error::FillError;
use crate::form::{FieldRef, Kind, RawValue};
use crate::pipeline::Pipeline;

/// Set the text of a text field (single, multi-line, comb, password).
///
/// Runs the field's keystroke and validate scripts first (a refusal fails the command with
/// [`FillError::Rejected`]; use [`crate::preflight_text`] to get the alerts beforehand), stores
/// the raw value in `/V`, recalculates, and writes the formatted appearance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetTextValue {
    pub field: FieldRef,
    pub value: String,
    #[serde(default = "super::yes")]
    pub run_scripts: bool,
}

impl SetTextValue {
    pub const NAME: &'static str = "fill.set_text";

    pub fn new(field: impl Into<FieldRef>, value: impl Into<String>) -> Self {
        SetTextValue {
            field: field.into(),
            value: value.into(),
            run_scripts: true,
        }
    }
}

/// Normalise user text for a field: single-line fields take no line breaks, multi-line ones
/// store `\r` as Acrobat does.
pub(crate) fn normalise(multiline: bool, v: &str) -> String {
    if multiline {
        v.replace("\r\n", "\r").replace('\n', "\r")
    } else {
        v.replace("\r\n", " ").replace(['\r', '\n'], " ")
    }
}

impl Command for SetTextValue {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.fill.set-text").arg("field", &self.field)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let mut p = Pipeline::begin(cx)?;
        let f = p.field(&self.field)?;
        if f.kind != Kind::Text {
            return Err(FillError::invalid(&f.name, "not a text field").into());
        }
        let value = normalise(f.multiline(), &self.value);
        if let Some(max) = f.max_len
            && value.chars().count() > max as usize
        {
            return Err(FillError::invalid(&f.name, format!("longer than MaxLen {max}")).into());
        }
        if self.run_scripts {
            let scripts = p.scripts_of(&f);
            let lookup = p.form_values();
            let v = engine::check_commit(&f, &scripts, &value, &lookup, &p.env);
            if !v.accepted {
                return Err(FillError::Rejected(v.alerts).into());
            }
        }
        p.write_value(&f, &RawValue::Text(value))?;
        p.finish()?;
        cx.changeset()
    }
}
