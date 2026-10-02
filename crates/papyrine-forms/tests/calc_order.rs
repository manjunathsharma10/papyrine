//! Calculation order (`/CO`): each calculate script runs once, in order, and
//! later scripts see earlier results.

use std::collections::HashMap;

use papyrine_forms::calc::{FormValues, MapForm, run_calculation_order};
use papyrine_forms::{DateEnv, recognize};

fn scripts() -> HashMap<String, Vec<papyrine_forms::Call>> {
    let mut m = HashMap::new();
    m.insert(
        "sub".to_string(),
        recognize("AFSimple_Calculate(\"SUM\", new Array(\"a\", \"b\"));").unwrap(),
    );
    m.insert(
        "total".to_string(),
        recognize("AFSimple_Calculate(\"SUM\", new Array(\"sub\", \"c\"));").unwrap(),
    );
    m.insert(
        "avg".to_string(),
        recognize("AFSimple_Calculate(\"AVG\", new Array(\"sub\", \"total\"));").unwrap(),
    );
    m
}

fn form() -> MapForm {
    let mut f = MapForm::default();
    for (k, v) in [
        ("a", "1"),
        ("b", "2"),
        ("c", "10"),
        ("sub", ""),
        ("total", ""),
        ("avg", ""),
    ] {
        f.set(k, v.to_string());
    }
    f
}

fn order(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn dependent_fields_calculate_in_co_order() {
    let mut f = form();
    let changed = run_calculation_order(
        &order(&["sub", "total", "avg"]),
        &scripts(),
        &mut f,
        &DateEnv::default(),
    );
    assert_eq!(changed, ["sub", "total", "avg"]);
    assert_eq!(f.get("sub").as_deref(), Some("3"));
    assert_eq!(f.get("total").as_deref(), Some("13"));
    assert_eq!(f.get("avg").as_deref(), Some("8"));
}

#[test]
fn wrong_order_gives_stale_results() {
    let mut f = form();
    run_calculation_order(
        &order(&["total", "sub"]),
        &scripts(),
        &mut f,
        &DateEnv::default(),
    );
    assert_eq!(
        f.get("total").as_deref(),
        Some("10"),
        "total ran before sub existed"
    );
    assert_eq!(f.get("sub").as_deref(), Some("3"));
}

#[test]
fn fields_outside_co_are_not_recalculated() {
    let mut f = form();
    let changed = run_calculation_order(&order(&["sub"]), &scripts(), &mut f, &DateEnv::default());
    assert_eq!(changed, ["sub"]);
    assert_eq!(f.get("total").as_deref(), Some(""));
}

#[test]
fn unchanged_values_are_not_reported() {
    let mut f = form();
    run_calculation_order(&order(&["sub"]), &scripts(), &mut f, &DateEnv::default());
    let again = run_calculation_order(&order(&["sub"]), &scripts(), &mut f, &DateEnv::default());
    assert!(again.is_empty());
}

#[test]
fn a_throwing_script_leaves_its_field_alone() {
    let mut f = form();
    let mut s = scripts();
    s.insert(
        "sub".into(),
        recognize("AFSimple_Calculate(\"BOGUS\", new Array(\"a\"));").unwrap(),
    );
    let changed = run_calculation_order(&order(&["sub", "total"]), &s, &mut f, &DateEnv::default());
    assert_eq!(changed, ["total"]);
    assert_eq!(f.get("sub").as_deref(), Some(""));
}
