//! Read-only entry points for the host UI: what committing a value would do (alerts, display
//! text, recalculated fields), live keystroke filtering, and the form status (read-only notice
//! for dynamic XFA, script banner).

use papyrine_cos::Document;
use papyrine_forms::{Alert, Blocker, TextColor};

use crate::engine::{self, DocForm, FieldScripts, Script, ScriptTable};
use crate::error::Result;
use crate::form::{FieldRef, FormTree, Kind, XfaState};

/// What committing `value` into a text field would do.
#[derive(Debug, Clone, PartialEq)]
pub struct Preflight {
    /// Keystroke and validate scripts accept the value.
    pub accepted: bool,
    pub alerts: Vec<Alert>,
    /// The text the widget will show (after the format script).
    pub display: String,
    pub color: Option<TextColor>,
    /// Calculated fields that would change: (qualified name, new value, new display).
    pub calculated: Vec<(String, String, String)>,
}

pub fn preflight_text(doc: &Document, field: &FieldRef, value: &str) -> Result<Preflight> {
    let form = FormTree::load(doc)?;
    let f = form.resolve(field)?.clone();
    let scripts = ScriptTable::load(&form);
    let fs = scripts
        .get(f.id)
        .cloned()
        .unwrap_or_else(FieldScripts::none);
    let env = engine::today();
    let mut df = DocForm::from_tree(&form);
    let verdict = engine::check_commit(&f, &fs, value, &df, &env);
    let fm = engine::format_value(&f, &fs, value, &df, &env);
    let mut calculated = Vec::new();
    if verdict.accepted && !form.calc_order.is_empty() {
        papyrine_forms::FormValues::set(&mut df, &f.name, value.to_string());
        let mut order = Vec::new();
        let mut calls = std::collections::HashMap::new();
        for id in &form.calc_order {
            if let Some(cf) = form.by_id(*id) {
                order.push(cf.name.clone());
                if let Some(FieldScripts {
                    calculate: Script::Native(c),
                    ..
                }) = scripts.get(cf.id)
                {
                    calls.insert(cf.name.clone(), c.clone());
                }
            }
        }
        for name in papyrine_forms::run_calculation_order(&order, &calls, &mut df, &env) {
            if let Some(cf) = form.by_name(&name) {
                let v = papyrine_forms::FormValues::get(&df, &name).unwrap_or_default();
                let cfs = scripts
                    .get(cf.id)
                    .cloned()
                    .unwrap_or_else(FieldScripts::none);
                let disp = engine::format_value(cf, &cfs, &v, &df, &env).display;
                calculated.push((name, v, disp));
            }
        }
    }
    Ok(Preflight {
        accepted: verdict.accepted,
        alerts: verdict.alerts,
        display: fm.display,
        color: fm.color,
        calculated,
    })
}

/// Live keystroke filtering for a text field (`value` is the text before the change,
/// `sel` the selection being replaced).
pub fn keystroke_filter(
    doc: &Document,
    field: &FieldRef,
    value: &str,
    change: &str,
    sel: (usize, usize),
) -> Result<engine::KeystrokeResult> {
    let form = FormTree::load(doc)?;
    let f = form.resolve(field)?;
    let scripts = ScriptTable::load(&form);
    let fs = scripts
        .get(f.id)
        .cloned()
        .unwrap_or_else(FieldScripts::none);
    Ok(engine::keystroke(
        f,
        &fs,
        value,
        change,
        sel,
        &DocForm::from_tree(&form),
        &engine::today(),
    ))
}

/// A script that did not run, for the banner.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedScript {
    pub field: String,
    /// `K`, `F`, `V` or `C`.
    pub trigger: &'static str,
    pub blocker: Blocker,
    pub reason: String,
}

/// Summary the UI uses for notices and banners.
#[derive(Debug, Clone, PartialEq)]
pub struct FormStatus {
    pub has_form: bool,
    pub field_count: usize,
    pub xfa: XfaState,
    /// Dynamic XFA: show a read-only notice; filling commands refuse.
    pub dynamic_xfa: bool,
    pub need_appearances: bool,
    /// Event scripts that exist and did not run (banner when non-empty).
    pub skipped_scripts: Vec<SkippedScript>,
    /// Event scripts that ran natively (or were no-ops).
    pub native_scripts: usize,
}

pub fn form_status(doc: &Document) -> Result<FormStatus> {
    let form = FormTree::load(doc)?;
    let scripts = ScriptTable::load(&form);
    let mut skipped = Vec::new();
    let mut native = 0;
    for f in &form.fields {
        let Some(fs) = scripts.get(f.id) else {
            continue;
        };
        for (trigger, s) in [
            ("K", &fs.keystroke),
            ("F", &fs.format),
            ("V", &fs.validate),
            ("C", &fs.calculate),
        ] {
            match s {
                Script::Unsupported(u) => skipped.push(SkippedScript {
                    field: f.name.clone(),
                    trigger,
                    blocker: u.blocker,
                    reason: u.reason.clone(),
                }),
                Script::Native(_) | Script::Noop => native += 1,
                Script::None => {}
            }
        }
    }
    Ok(FormStatus {
        has_form: form.has_form(),
        field_count: form.fields.len(),
        xfa: form.xfa,
        dynamic_xfa: form.xfa == XfaState::Dynamic,
        need_appearances: form.need_appearances,
        skipped_scripts: skipped,
        native_scripts: native,
    })
}

/// A field as the UI lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldSummary {
    pub id: papyrine_cos::ObjId,
    pub name: String,
    pub kind: Kind,
    pub read_only: bool,
    pub widgets: usize,
}

pub fn list_fields(doc: &Document) -> Result<Vec<FieldSummary>> {
    let form = FormTree::load(doc)?;
    Ok(form
        .fields
        .iter()
        .map(|f| FieldSummary {
            id: f.id,
            name: f.name.clone(),
            kind: f.kind,
            read_only: f.is_read_only(),
            widgets: f.widgets.len(),
        })
        .collect())
}
