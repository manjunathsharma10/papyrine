use papyrine_ops::{ChangeSet, Command, EditContext, LocalizedText, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::FillError;
use crate::form::{Field, FieldRef, Kind, RawValue, flags};
use crate::pipeline::{Pipeline, on_state_name};

/// Whether widget `idx` of a button field currently shows its on state.
fn widget_is_on(f: &Field, idx: usize) -> bool {
    let w = &f.widgets[idx];
    match w.appearance_state.as_deref() {
        Some(s) => s != "Off",
        None => match &f.value {
            RawValue::Name(n) | RawValue::Text(n) => *n == on_state_name(f, idx),
            _ => false,
        },
    }
}

/// Tick, untick or toggle a check box.
///
/// `widget` selects the widget when a field has several (they may have different export values);
/// widgets sharing the chosen on-state follow it. `checked: None` toggles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToggleCheckbox {
    pub field: FieldRef,
    #[serde(default)]
    pub widget: usize,
    #[serde(default)]
    pub checked: Option<bool>,
}

impl ToggleCheckbox {
    pub const NAME: &'static str = "fill.toggle_checkbox";

    pub fn new(field: impl Into<FieldRef>, checked: Option<bool>) -> Self {
        ToggleCheckbox {
            field: field.into(),
            widget: 0,
            checked,
        }
    }
}

impl Command for ToggleCheckbox {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.fill.toggle-checkbox").arg("field", &self.field)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let mut p = Pipeline::begin(cx)?;
        let f = p.field(&self.field)?;
        if f.kind != Kind::Checkbox {
            return Err(FillError::invalid(&f.name, "not a check box").into());
        }
        if self.widget >= f.widgets.len() {
            return Err(FillError::invalid(&f.name, "no such widget").into());
        }
        let on_name = on_state_name(&f, self.widget);
        let target = self
            .checked
            .unwrap_or_else(|| !widget_is_on(&f, self.widget));
        let value = if target {
            RawValue::Name(on_name.clone())
        } else {
            RawValue::Name("Off".into())
        };
        p.write_value(&f, &value)?;
        for (i, w) in f.widgets.iter().enumerate() {
            let on_here = target && on_state_name(&f, i) == on_name;
            p.ensure_button_aps(&f, i, w)?;
            let state = if on_here {
                on_state_name(&f, i)
            } else {
                "Off".into()
            };
            p.set_as(w, &state)?;
        }
        p.finish()?;
        cx.changeset()
    }
}

/// Select (or, for groups that allow it, clear) a radio button.
///
/// Name the button by `export` value or by `widget` index. Selecting the already-selected button
/// clears the group unless the field has *NoToggleToOff*. With *RadiosInUnison* every widget that
/// shares the chosen export value turns on; otherwise only the chosen widget does.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectRadio {
    pub field: FieldRef,
    #[serde(default)]
    pub export: Option<String>,
    #[serde(default)]
    pub widget: Option<usize>,
}

impl SelectRadio {
    pub const NAME: &'static str = "fill.select_radio";

    pub fn by_export(field: impl Into<FieldRef>, export: impl Into<String>) -> Self {
        SelectRadio {
            field: field.into(),
            export: Some(export.into()),
            widget: None,
        }
    }

    pub fn by_widget(field: impl Into<FieldRef>, widget: usize) -> Self {
        SelectRadio {
            field: field.into(),
            export: None,
            widget: Some(widget),
        }
    }
}

impl Command for SelectRadio {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.fill.select-radio").arg("field", &self.field)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let mut p = Pipeline::begin(cx)?;
        let f = p.field(&self.field)?;
        if f.kind != Kind::Radio {
            return Err(FillError::invalid(&f.name, "not a radio button group").into());
        }
        let idx = match (&self.widget, &self.export) {
            (Some(i), _) if *i < f.widgets.len() => *i,
            (Some(_), _) => return Err(FillError::invalid(&f.name, "no such widget").into()),
            (None, Some(e)) => (0..f.widgets.len())
                .find(|&i| on_state_name(&f, i) == *e)
                .ok_or_else(|| FillError::invalid(&f.name, format!("no button exports `{e}`")))?,
            (None, None) => {
                return Err(
                    FillError::invalid(&f.name, "name a button by export or widget").into(),
                );
            }
        };
        let on_name = on_state_name(&f, idx);
        if on_name == "Off" {
            return Err(FillError::invalid(&f.name, "a button cannot export `Off`").into());
        }
        let selected_now = matches!(&f.value, RawValue::Name(n) | RawValue::Text(n) if *n == on_name)
            && widget_is_on(&f, idx);
        let no_toggle_off = f.has(flags::NO_TOGGLE_TO_OFF);
        let unison = f.has(flags::RADIOS_IN_UNISON);
        let turn_on = !(selected_now && !no_toggle_off);
        if selected_now && no_toggle_off {
            // Nothing changes; still a valid (empty) step.
            return cx.changeset();
        }
        let value = if turn_on {
            RawValue::Name(on_name.clone())
        } else {
            RawValue::Name("Off".into())
        };
        p.write_value(&f, &value)?;
        for (i, w) in f.widgets.iter().enumerate() {
            p.ensure_button_aps(&f, i, w)?;
            let here = turn_on && ((unison && on_state_name(&f, i) == on_name) || i == idx);
            let state = if here {
                on_state_name(&f, i)
            } else {
                "Off".into()
            };
            p.set_as(w, &state)?;
        }
        p.finish()?;
        cx.changeset()
    }
}
