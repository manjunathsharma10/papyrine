//! The event model over the native AF subset (ADR-006, ADR-030).
//!
//! Scripts are read from each field's `/AA` (`K` keystroke, `F` format, `V` validate,
//! `C` calculate), classified once with the strict recognizer and then executed per script:
//! accepted scripts run, Adobe boilerplate and empty scripts are no-ops, everything else is
//! inert and listed for the banner. Nothing here runs JavaScript.
//!
//! Event order on committing a value (Acrobat's): keystroke (`willCommit`), validate, store the
//! raw value, calculate in `/CO` order, format for display.

use std::collections::HashMap;

use papyrine_cos::{ObjId, Object, ObjectKind};
pub use papyrine_forms::FormValues;
use papyrine_forms::{
    AfError, Alert, Blocker, Call, DateEnv, Event, FieldLookup, TextColor, Verdict, analyze,
    datefmt::DateTime, execute,
};

use crate::cosx;
use crate::form::{Field, FormTree, Kind, RawValue};

/// One classified script.
#[derive(Debug, Clone, PartialEq)]
pub enum Script {
    /// The trigger has no JavaScript action.
    None,
    Native(Vec<Call>),
    /// Empty script or known Adobe boilerplate: nothing to do.
    Noop,
    /// Outside the native subset; never run.
    Unsupported(Unsupported),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Unsupported {
    pub source: String,
    pub blocker: Blocker,
    pub reason: String,
}

impl Script {
    fn from_source(src: &str) -> Script {
        match analyze(src) {
            Verdict::Empty | Verdict::Boilerplate(_) => Script::Noop,
            Verdict::Accepted(calls) => Script::Native(calls),
            Verdict::Rejected { reason, blocker } => Script::Unsupported(Unsupported {
                source: src.to_string(),
                blocker,
                reason: reason.to_string(),
            }),
        }
    }

    pub fn is_unsupported(&self) -> bool {
        matches!(self, Script::Unsupported(_))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FieldScripts {
    pub keystroke: Script,
    pub format: Script,
    pub validate: Script,
    pub calculate: Script,
}

impl FieldScripts {
    pub fn none() -> Self {
        FieldScripts {
            keystroke: Script::None,
            format: Script::None,
            validate: Script::None,
            calculate: Script::None,
        }
    }
}

/// The JavaScript of an action dictionary and its `/Next` chain (JavaScript steps only).
pub fn action_javascript(action: &Object) -> Option<String> {
    let mut out = String::new();
    let mut stack = vec![action.clone()];
    let mut guard = 0;
    while let Some(a) = stack.pop() {
        guard += 1;
        if guard > 16 {
            break;
        }
        if !cosx::is_dict(&a) {
            continue;
        }
        if cosx::get_name(&a, "S").as_deref() == Some("JavaScript")
            && let Some(js) = cosx::get(&a, "JS")
            && let Some(t) = cosx::text(&js)
        {
            if !out.is_empty() {
                out.push_str(";\n");
            }
            out.push_str(&t);
        }
        if let Some(next) = cosx::get(&a, "Next") {
            match cosx::kind(&next) {
                Some(ObjectKind::Array) => {
                    let mut items = cosx::items(&next);
                    items.reverse();
                    stack.extend(items);
                }
                _ => stack.push(next),
            }
        }
    }
    (!out.is_empty() || cosx::get_name(action, "S").as_deref() == Some("JavaScript")).then_some(out)
}

/// Classify the four event scripts of a field (on the field node and its widgets).
pub fn field_scripts(field: &Field) -> FieldScripts {
    let mut fs = FieldScripts::none();
    let mut holders = vec![field.obj.clone()];
    holders.extend(field.widgets.iter().map(|w| w.obj.clone()));
    for h in holders {
        let Some(aa) = cosx::get(&h, "AA") else {
            continue;
        };
        for (key, slot) in [
            ("K", &mut fs.keystroke),
            ("F", &mut fs.format),
            ("V", &mut fs.validate),
            ("C", &mut fs.calculate),
        ] {
            if !matches!(slot, Script::None) {
                continue;
            }
            if let Some(src) = cosx::get(&aa, key).and_then(|a| action_javascript(&a)) {
                *slot = Script::from_source(&src);
            }
        }
    }
    fs
}

/// Scripts of every field, by object id.
pub struct ScriptTable {
    pub by_field: HashMap<ObjId, FieldScripts>,
}

impl ScriptTable {
    pub fn load(form: &FormTree) -> ScriptTable {
        ScriptTable {
            by_field: form
                .fields
                .iter()
                .map(|f| (f.id, field_scripts(f)))
                .collect(),
        }
    }

    pub fn get(&self, id: ObjId) -> Option<&FieldScripts> {
        self.by_field.get(&id)
    }
}

/// Today's date (UTC) for `AFDate_*` functions that default missing parts.
pub fn today() -> DateEnv {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let days = (secs / 86_400) as i64;
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y } as i32;
    DateEnv {
        today: DateTime::ymd(y, m, d),
    }
}

/// Field values as the calculation functions see them: by qualified name; `None` for fields with
/// no numeric content (unchecked boxes, buttons, multi-select lists).
#[derive(Debug, Default, Clone)]
pub struct DocForm {
    order: Vec<String>,
    values: HashMap<String, Option<String>>,
}

impl DocForm {
    pub fn from_tree(form: &FormTree) -> DocForm {
        let mut d = DocForm::default();
        for f in &form.fields {
            let v = calc_value(f);
            if d.values.insert(f.name.clone(), v).is_none() && !d.order.contains(&f.name) {
                d.order.push(f.name.clone());
            }
        }
        d
    }
}

/// The value of a field for calculations.
pub fn calc_value(f: &Field) -> Option<String> {
    match f.kind {
        Kind::Text | Kind::Combo => Some(f.value.as_text().unwrap_or("").to_string()),
        Kind::List => match &f.value {
            RawValue::List(l) if l.len() != 1 => None,
            v => Some(v.as_text().unwrap_or("").to_string()),
        },
        Kind::Checkbox | Kind::Radio => match &f.value {
            RawValue::Name(n) if n != "Off" => Some(n.clone()),
            _ => None,
        },
        _ => None,
    }
}

impl FieldLookup for DocForm {
    fn values(&self, name: &str) -> Vec<Option<String>> {
        let prefix = format!("{name}.");
        self.order
            .iter()
            .filter(|k| k.as_str() == name || k.starts_with(&prefix))
            .map(|k| self.values[k].clone())
            .collect()
    }
}

impl FormValues for DocForm {
    fn get(&self, name: &str) -> Option<String> {
        self.values.get(name).cloned().flatten()
    }
    fn set(&mut self, name: &str, value: String) {
        if self.values.insert(name.to_string(), Some(value)).is_none() {
            self.order.push(name.to_string());
        }
    }
}

/// Result of the format event.
#[derive(Debug, Clone, PartialEq)]
pub struct Formatted {
    pub display: String,
    pub color: Option<TextColor>,
    /// A format script ran (as opposed to the raw value being shown as is).
    pub ran: bool,
}

/// Run the format script (if any) over `value`.
pub fn format_value(
    field: &Field,
    scripts: &FieldScripts,
    value: &str,
    lookup: &dyn FieldLookup,
    env: &DateEnv,
) -> Formatted {
    let plain = Formatted {
        display: value.to_string(),
        color: None,
        ran: false,
    };
    let Script::Native(calls) = &scripts.format else {
        return plain;
    };
    let mut ev = Event::format(value);
    ev.target_name = field.name.clone();
    match execute(calls, &mut ev, lookup, env) {
        Ok(()) => Formatted {
            display: ev.value,
            color: ev.text_color,
            ran: true,
        },
        Err(AfError::Param | AfError::Value) => plain,
    }
}

/// Outcome of the keystroke + validate events for a value about to be committed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Verdicts {
    pub accepted: bool,
    pub alerts: Vec<Alert>,
}

/// Run `K` (willCommit) then `V` over `value`. A throwing script is treated as accepting.
pub fn check_commit(
    field: &Field,
    scripts: &FieldScripts,
    value: &str,
    lookup: &dyn FieldLookup,
    env: &DateEnv,
) -> Verdicts {
    let mut v = Verdicts {
        accepted: true,
        alerts: Vec::new(),
    };
    for s in [&scripts.keystroke, &scripts.validate] {
        let Script::Native(calls) = s else { continue };
        let mut ev = Event::commit(value);
        ev.target_name = field.name.clone();
        if execute(calls, &mut ev, lookup, env).is_ok() {
            v.alerts.append(&mut ev.alerts);
            if !ev.rc {
                v.accepted = false;
                break;
            }
        }
    }
    v
}

/// Per-keystroke filter for live typing: the change the field should accept, or `None` to
/// reject the keystroke.
#[derive(Debug, Clone, PartialEq)]
pub struct KeystrokeResult {
    pub accepted: bool,
    /// The (possibly rewritten) text to insert.
    pub change: String,
    pub alerts: Vec<Alert>,
}

pub fn keystroke(
    field: &Field,
    scripts: &FieldScripts,
    value: &str,
    change: &str,
    sel: (usize, usize),
    lookup: &dyn FieldLookup,
    env: &DateEnv,
) -> KeystrokeResult {
    let mut out = KeystrokeResult {
        accepted: true,
        change: change.to_string(),
        alerts: Vec::new(),
    };
    if let Script::Native(calls) = &scripts.keystroke {
        let mut ev = Event::keystroke(value, change, sel.0, sel.1);
        ev.target_name = field.name.clone();
        if execute(calls, &mut ev, lookup, env).is_ok() {
            out.accepted = ev.rc;
            out.change = ev.change;
            out.alerts = ev.alerts;
        }
    }
    out
}
