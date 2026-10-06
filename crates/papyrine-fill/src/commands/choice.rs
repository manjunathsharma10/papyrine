use papyrine_ops::{ChangeSet, Command, EditContext, LocalizedText, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::appearance::parse_da;
use crate::engine;
use crate::error::FillError;
use crate::form::{Field, FieldRef, Kind, RawValue};
use crate::pipeline::Pipeline;

/// Choose values in a combo box or list box.
///
/// `values` are export values (a display text is accepted when it is unambiguous). A combo box
/// takes at most one value, and with the *Edit* flag any text; a single-select list box at most
/// one; a multi-select list box any number. An empty list clears the field. `top_index` scrolls
/// a list box (`/TI`); by default it is adjusted so the first selection is visible.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetChoice {
    pub field: FieldRef,
    pub values: Vec<String>,
    #[serde(default)]
    pub top_index: Option<u32>,
    #[serde(default = "super::yes")]
    pub run_scripts: bool,
}

impl SetChoice {
    pub const NAME: &'static str = "fill.set_choice";

    pub fn new(field: impl Into<FieldRef>, values: Vec<String>) -> Self {
        SetChoice {
            field: field.into(),
            values,
            top_index: None,
            run_scripts: true,
        }
    }
}

/// Map a requested value to an export value of the field's options.
fn to_export(f: &Field, v: &str) -> Option<String> {
    if f.options.iter().any(|(e, _)| e == v) {
        return Some(v.to_string());
    }
    let mut hits = f.options.iter().filter(|(_, d)| d == v);
    match (hits.next(), hits.next()) {
        (Some((e, _)), None) => Some(e.clone()),
        _ => None,
    }
}

/// First row index of a list box that keeps `first` visible.
fn visible_top(f: &Field, first: usize, current: Option<u32>) -> u32 {
    let w = f.widgets.first();
    let size = parse_da(f.da.as_deref()).size;
    let size = if size > 0.0 { size } else { 12.0 };
    let rows = w.map_or(1, |w| {
        (((w.height() - 2.0 * w.border_width) / (size * 1.15)).floor() as usize).max(1)
    });
    let top = current.unwrap_or(0) as usize;
    if first < top {
        first as u32
    } else if first >= top + rows {
        (first + 1 - rows) as u32
    } else {
        top as u32
    }
}

impl Command for SetChoice {
    fn name(&self) -> &'static str {
        Self::NAME
    }

    fn describe(&self) -> LocalizedText {
        LocalizedText::new("cmd.fill.set-choice").arg("field", &self.field)
    }

    fn params(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    fn apply(&mut self, cx: &mut EditContext<'_>) -> Result<ChangeSet> {
        let mut p = Pipeline::begin(cx)?;
        let f = p.field(&self.field)?;
        if !matches!(f.kind, Kind::Combo | Kind::List) {
            return Err(FillError::invalid(&f.name, "not a choice field").into());
        }
        let multi = f.multi_select();
        if !multi && self.values.len() > 1 {
            return Err(FillError::invalid(&f.name, "this field allows one value").into());
        }
        let mut chosen: Vec<String> = Vec::new();
        for v in &self.values {
            match to_export(&f, v) {
                Some(e) => chosen.push(e),
                None if f.editable_combo() => chosen.push(v.clone()),
                None => {
                    return Err(
                        FillError::invalid(&f.name, format!("`{v}` is not an option")).into(),
                    );
                }
            }
        }
        // Keep the options' order for multi-select lists.
        if multi {
            chosen.sort_by_key(|e| f.options.iter().position(|(x, _)| x == e));
            chosen.dedup();
        }
        if self.run_scripts && f.kind == Kind::Combo {
            let scripts = p.scripts_of(&f);
            let lookup = p.form_values();
            let text = chosen.first().cloned().unwrap_or_default();
            let v = engine::check_commit(&f, &scripts, &text, &lookup, &p.env);
            if !v.accepted {
                return Err(FillError::Rejected(v.alerts).into());
            }
        }
        let value = match chosen.len() {
            0 => RawValue::None,
            1 => RawValue::Text(chosen[0].clone()),
            _ => RawValue::List(chosen.clone()),
        };
        p.write_value(&f, &value)?;
        let doc = p.cx.doc();
        if f.kind == Kind::List {
            let idx: Vec<usize> = chosen
                .iter()
                .filter_map(|e| f.options.iter().position(|(x, _)| x == e))
                .collect();
            if idx.is_empty() {
                p.cx.remove_key(&f.obj, "I")?;
            } else {
                let a = doc.new_array();
                let mut sorted = idx.clone();
                sorted.sort_unstable();
                for i in sorted {
                    a.array_push(&doc.new_int(i as i64))?;
                }
                p.cx.set_key(&f.obj, "I", &a)?;
            }
            let top = self.top_index.or_else(|| {
                idx.iter()
                    .min()
                    .map(|&first| visible_top(&f, first, f.top_index))
            });
            if let Some(t) = top.filter(|&t| t > 0 || f.top_index.is_some()) {
                p.cx.set_key(&f.obj, "TI", &doc.new_int(i64::from(t)))?;
            }
        } else if f.obj.dict_has("I")? {
            p.cx.remove_key(&f.obj, "I")?;
        }
        p.finish()?;
        cx.changeset()
    }
}
