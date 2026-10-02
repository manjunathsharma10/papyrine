//! `AFSimple_Calculate`, `AFRange_Validate` and calculation-order driving.

use std::collections::HashMap;

use crate::event::{AfError, Alert, Event, FieldLookup};
use crate::numfmt::{atof, js_number_to_string, make_number};
use crate::recognize::Call;

/// Split a comma-separated field list into trimmed names.
pub fn split_field_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// `AFSimple_Calculate(func, fields)`: SUM, AVG, PRD, MIN or MAX over the
/// numeric values of the named fields. The result (rounded to six decimals)
/// becomes `event.value`.
pub fn simple_calculate(
    ev: &mut Event,
    func: &str,
    names: &[String],
    fields: &dyn FieldLookup,
) -> Result<(), AfError> {
    let f = func.to_ascii_uppercase();
    if !matches!(f.as_str(), "SUM" | "AVG" | "PRD" | "MIN" | "MAX") {
        // Acrobat only complains once it has a value to combine.
        let any = names.iter().any(|n| !fields.values(n).is_empty());
        if any {
            return Err(AfError::Value);
        }
    }
    let mut acc = if f == "PRD" { 1.0 } else { 0.0 };
    let mut count = 0usize;
    for name in names {
        for v in fields.values(name) {
            let x = v.as_deref().map_or(0.0, make_number);
            acc = match f.as_str() {
                "MIN" if count == 0 => x,
                "MAX" if count == 0 => x,
                "MIN" => acc.min(x),
                "MAX" => acc.max(x),
                "PRD" => acc * x,
                _ => acc + x,
            };
            count += 1;
        }
    }
    if f == "AVG" && count > 0 {
        acc /= count as f64;
    }
    acc = (acc * 1e6 + 0.49).floor() / 1e6;
    if acc == 0.0 {
        acc = 0.0; // normalise -0
    }
    ev.value = js_number_to_string(acc);
    Ok(())
}

/// `AFRange_Validate(bGreaterThan, nGreaterThan, bLessThan, nLessThan)`.
pub fn range_validate(ev: &mut Event, gt: bool, lo: f64, lt: bool, hi: f64) {
    if ev.value.is_empty() {
        return;
    }
    let x = atof(&ev.value);
    let lo_s = js_number_to_string(lo);
    let hi_s = js_number_to_string(hi);
    let alert = if gt && lt {
        (x < lo || x > hi).then_some(Alert::RangeBetween { lo: lo_s, hi: hi_s })
    } else if gt {
        (x < lo).then_some(Alert::RangeGreater { lo: lo_s })
    } else if lt {
        (x > hi).then_some(Alert::RangeLess { hi: hi_s })
    } else {
        None
    };
    if let Some(a) = alert {
        ev.alerts.push(a);
        ev.rc = false;
    }
}

/// Values the calculation driver can read and write.
pub trait FormValues: FieldLookup {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&mut self, name: &str, value: String);
}

/// A simple map-backed form for tests and for hosts without their own model.
#[derive(Debug, Default, Clone)]
pub struct MapForm(pub HashMap<String, String>);

impl FieldLookup for MapForm {
    fn values(&self, name: &str) -> Vec<Option<String>> {
        // A parent name ("Sched") covers its children ("Sched.a", "Sched.b").
        let prefix = format!("{name}.");
        let mut keys: Vec<&String> = self
            .0
            .keys()
            .filter(|k| k.as_str() == name || k.starts_with(&prefix))
            .collect();
        keys.sort();
        keys.into_iter().map(|k| Some(self.0[k].clone())).collect()
    }
}

impl FormValues for MapForm {
    fn get(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
    fn set(&mut self, name: &str, value: String) {
        self.0.insert(name.to_string(), value);
    }
}

/// Run each field's calculate script once, in `/CO` order. A field not named
/// in `order` is not recalculated. Returns the names whose value changed.
/// A script that throws leaves its field untouched.
pub fn run_calculation_order<F: FormValues>(
    order: &[String],
    scripts: &HashMap<String, Vec<Call>>,
    form: &mut F,
    env: &crate::datefmt::DateEnv,
) -> Vec<String> {
    let mut changed = Vec::new();
    for name in order {
        let Some(calls) = scripts.get(name) else {
            continue;
        };
        let mut ev = Event::format(&form.get(name).unwrap_or_default());
        ev.target_name = name.clone();
        if crate::exec::execute(calls, &mut ev, &*form, env).is_ok()
            && ev.rc
            && form.get(name).as_deref() != Some(ev.value.as_str())
        {
            form.set(name, ev.value);
            changed.push(name.clone());
        }
    }
    changed
}
