use papyrine_ops::{ChangeSet, Command, EditContext, LocalizedText, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::form::{Field, FieldRef, Kind, RawValue};
use crate::pipeline::{Pipeline, on_state_name};

/// Reset fields to their default values (`/DV`), or clear them when there is none.
///
/// `fields: None` resets every field. With `exclude: true` the named fields are the ones left
/// alone. Read-only fields, push buttons and signature fields are never reset (Acrobat's
/// *Clear Form* leaves them too).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResetForm {
    #[serde(default)]
    pub fields: Option<Vec<FieldRef>>,
    #[serde(default)]
    pub exclude: bool,
}

impl ResetForm {
    pub const NAME: &'static str = "fill.reset_form";

    pub fn all() -> Self {
        ResetForm::default()
    }
}

impl Command for ResetForm {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.fill.reset-form")
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let mut p = Pipeline::begin(cx)?;
        // Resolve the selection to field ids first.
        let named: Option<Vec<_>> = match &self.fields {
            Some(list) => Some(
                list.iter()
                    .map(|r| p.form.resolve(r).map(|f| f.id))
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            ),
            None => None,
        };
        let targets: Vec<Field> = p
            .form
            .fields
            .iter()
            .filter(|f| {
                let hit = named.as_ref().is_none_or(|ids| ids.contains(&f.id));
                let hit = if self.exclude && named.is_some() {
                    !hit
                } else {
                    hit
                };
                hit && !f.is_read_only()
                    && !matches!(f.kind, Kind::PushButton | Kind::Signature | Kind::Other)
            })
            .cloned()
            .collect();
        for f in &targets {
            reset_field(&mut p, f)?;
        }
        p.finish()?;
        cx.changeset()
    }
}

fn reset_field(p: &mut Pipeline<'_, '_>, f: &Field) -> Result<()> {
    match f.kind {
        Kind::Checkbox | Kind::Radio => {
            let dv = match &f.default_value {
                RawValue::Name(n) | RawValue::Text(n) if n != "Off" => Some(n.clone()),
                _ => None,
            };
            let value = RawValue::Name(dv.clone().unwrap_or_else(|| "Off".into()));
            if f.value == value
                && f.widgets.iter().all(|w| {
                    (w.appearance_state.as_deref().is_some_and(|s| s != "Off")) == dv.is_some()
                })
                && f.value != RawValue::None
            {
                return Ok(());
            }
            p.write_value(f, &value)?;
            let mut used = false;
            for (i, w) in f.widgets.iter().enumerate() {
                let name = on_state_name(f, i);
                let on =
                    dv.as_deref() == Some(name.as_str()) && (f.kind == Kind::Checkbox || !used);
                if on {
                    used = true;
                }
                p.ensure_button_aps(f, i, w)?;
                p.set_as(w, if on { &name } else { "Off" })?;
            }
        }
        _ => {
            let cleared = f.value == RawValue::None || f.value == RawValue::Text(String::new());
            let target = f.default_value.clone();
            if (target == RawValue::None && cleared) || f.value == target {
                return Ok(());
            }
            p.write_value(f, &target)?;
            if f.kind == Kind::List {
                p.cx.remove_key(&f.obj, "I")?;
            }
        }
    }
    Ok(())
}
