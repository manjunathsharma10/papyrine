//! The 500-case AF reference suite of `papyrine-forms`, replayed end to end: every case that
//! goes through the event model (format, keystroke, validate, calculate) runs as a field script
//! in a real PDF, through `preflight_text`, `keystroke_filter` and the `SetTextValue` command,
//! and the resulting appearance stream is read back. Cases that exercise library functions with
//! no event (`AFMergeChange`, `AFMakeNumber`, date parsing) are counted separately and are
//! covered by the `papyrine-forms` suite itself.

mod common;

#[allow(dead_code, clippy::all)]
#[path = "../../papyrine-forms/tests/af_reference.rs"]
mod af_ref;

use common::*;
use papyrine_content::parse;
use papyrine_cos::{DecodeLevel, Document};
use papyrine_fill::engine::with_fixed_today;
use papyrine_fill::form::FormTree;
use papyrine_fill::*;
use papyrine_forms::TextColor;

/// A PDF string literal as UTF-16BE hex (any script text survives).
fn js_string(s: &str) -> String {
    let mut h = String::from("<FEFF");
    for u in s.encode_utf16() {
        h.push_str(&format!("{u:04X}"));
    }
    h.push('>');
    h
}

fn text_field(name: &str, extra: &str) -> String {
    format!(
        "<< /Type /Annot /Subtype /Widget /FT /Tx /T ({name}) /F 4 /DA (/Helv 12 Tf 0 g) /Rect [10 10 310 40] /MK << /BC [0] >> {extra} >>"
    )
}

/// One page, one field `f` carrying `trigger` (`K`, `F`, `V`) = `script`.
fn one_field(trigger: &str, script: &str) -> Vec<u8> {
    let mut p = Pdf::new();
    p.add("<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>");
    p.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    p.add("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 100] /Annots [4 0 R] /Resources << /Font << /Helv 6 0 R >> >> >>");
    p.add(text_field(
        "f",
        &format!(
            "/P 3 0 R /AA << /{trigger} << /S /JavaScript /JS {} >> >>",
            js_string(script)
        ),
    ));
    p.add("<< /Fields [4 0 R] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 6 0 R >> >> >>");
    p.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
    p.build(1)
}

/// `fields` pre-filled, a `trigger` field, and `calc` calculated by `script`.
fn calc_pdf(script: &str, fields: &[(&str, &str)]) -> Vec<u8> {
    let mut p = Pdf::new();
    p.add("<< /Type /Catalog /Pages 2 0 R /AcroForm 5 0 R >>");
    p.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    p.add("PAGE");
    p.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>");
    p.add("ACRO");
    let mut ids = Vec::new();
    let calc = p.add(text_field(
        "calc",
        &format!(
            "/P 3 0 R /V (~) /AA << /C << /S /JavaScript /JS {} >> >>",
            js_string(script)
        ),
    ));
    ids.push(calc);
    ids.push(p.add(text_field("trigger", "/P 3 0 R")));
    for (n, v) in fields {
        ids.push(p.add(text_field(n, &format!("/P 3 0 R /V {}", js_string(v)))));
    }
    let refs: Vec<String> = ids.iter().map(|i| format!("{i} 0 R")).collect();
    p.set(3, format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 100] /Annots [{}] /Resources << /Font << /Helv 4 0 R >> >> >>", refs.join(" ")));
    p.set(5, format!("<< /Fields [{}] /DA (/Helv 0 Tf 0 g) /DR << /Font << /Helv 4 0 R >> >> /CO [{calc} 0 R] >>", refs.join(" ")));
    p.build(1)
}

/// The string shown by the first `Tj`/`TJ` of a widget's appearance, and the fill colour ops.
fn shown(doc: &Document, field: &str) -> (String, String) {
    let t = FormTree::load(doc).unwrap();
    let f = t.by_name(field).unwrap();
    let ap = f.widgets[0]
        .obj
        .dict_get("AP")
        .unwrap()
        .dict_get("N")
        .unwrap();
    let data = ap
        .stream_decoded(DecodeLevel::Generalized)
        .unwrap()
        .to_vec();
    let ops = parse(&data).ops;
    let mut s = String::new();
    let mut colour = String::new();
    for op in &ops {
        match op.operator_str().as_ref() {
            "Tj" => {
                if let Some(b) = op.operands.first().and_then(|o| o.as_str_bytes()) {
                    s.extend(b.iter().map(|&c| c as char));
                }
            }
            "rg" | "g" if s.is_empty() => {
                colour = format!(
                    "{} {}",
                    op.operands
                        .iter()
                        .filter_map(|o| o.as_f64())
                        .map(|v| v.to_string())
                        .collect::<Vec<_>>()
                        .join(" "),
                    op.operator_str()
                );
            }
            _ => {}
        }
    }
    (s, colour)
}

fn printable_ascii(s: &str) -> bool {
    s.chars().all(|c| (' '..='~').contains(&c))
}

#[derive(Default)]
struct Counts {
    format: usize,
    key: usize,
    calc: usize,
    library_only: usize,
}

#[test]
fn reference_cases_pass_end_to_end_through_the_event_model() {
    let cases = af_ref::all_cases();
    let env = af_ref::env();
    let mut counts = Counts::default();
    let mut failures = Vec::new();
    for c in &cases {
        let r: Result<(), String> = with_fixed_today(env, || match &c.kind {
            af_ref::Kind::Format {
                script,
                init,
                expect,
                color,
            } => {
                counts.format += 1;
                (|| {
                    let doc = open(one_field("F", script));
                    let pf = preflight_text(&doc, &"f".into(), init).map_err(|e| e.to_string())?;
                    if &pf.display != expect {
                        return Err(format!("display {:?}, expected {expect:?}", pf.display));
                    }
                    if let Some(col) = color
                        && pf.color != Some(*col)
                    {
                        return Err(format!("colour {:?}, expected {col:?}", pf.color));
                    }
                    // Through the command: the raw value is stored, the display is drawn.
                    let mut h = history();
                    let cmd = SetTextValue {
                        field: "f".into(),
                        value: init.clone(),
                        run_scripts: false,
                    };
                    h.execute(&doc, Box::new(cmd)).map_err(|e| e.to_string())?;
                    let t = FormTree::load(&doc).unwrap();
                    if t.by_name("f").unwrap().value.as_text() != Some(init.as_str())
                        && !init.contains(['\n', '\r'])
                    {
                        return Err("raw value not stored as typed".into());
                    }
                    if printable_ascii(expect) && printable_ascii(init) && !expect.is_empty() {
                        let (drawn, colour) = shown(&doc, "f");
                        if &drawn != expect {
                            return Err(format!("appearance draws {drawn:?}, expected {expect:?}"));
                        }
                        match color {
                            Some(TextColor::Red) if colour != "1 0 0 rg" => {
                                return Err(format!("appearance colour {colour:?}"));
                            }
                            None if colour != "0 g" => {
                                return Err(format!("appearance colour {colour:?}"));
                            }
                            _ => {}
                        }
                    }
                    Ok(())
                })()
            }
            af_ref::Kind::Key {
                script,
                value,
                change,
                sel,
                commit,
                rc,
                change_out,
                alert,
            } => {
                counts.key += 1;
                (|| {
                    let doc = open(one_field("K", script));
                    if *commit {
                        let pf =
                            preflight_text(&doc, &"f".into(), value).map_err(|e| e.to_string())?;
                        if pf.accepted != *rc {
                            return Err(format!("accepted {}, expected {rc}", pf.accepted));
                        }
                        if *alert != !pf.alerts.is_empty() {
                            return Err(format!("alerts {:?}, expected alert={alert}", pf.alerts));
                        }
                        let mut h = history();
                        let r = h.execute(&doc, Box::new(SetTextValue::new("f", *value)));
                        if r.is_ok() != *rc {
                            return Err(format!("command result {r:?}, expected accept={rc}"));
                        }
                        if !*rc {
                            let e = r.unwrap_err().to_string();
                            if !e.contains("rejected") {
                                return Err(format!("unexpected error {e}"));
                            }
                            if h.can_undo() {
                                return Err("a rejected command left history".into());
                            }
                        }
                    } else {
                        let k = keystroke_filter(&doc, &"f".into(), value, change, *sel)
                            .map_err(|e| e.to_string())?;
                        if k.accepted != *rc {
                            return Err(format!("accepted {}, expected {rc}", k.accepted));
                        }
                        if let Some(co) = change_out
                            && k.change != *co
                        {
                            return Err(format!("change {:?}, expected {co:?}", k.change));
                        }
                    }
                    Ok(())
                })()
            }
            af_ref::Kind::Calc {
                script,
                fields,
                expect,
            } => {
                counts.calc += 1;
                (|| {
                    let doc = open(calc_pdf(script, fields));
                    let pf =
                        preflight_text(&doc, &"trigger".into(), "x").map_err(|e| e.to_string())?;
                    let got = pf
                        .calculated
                        .iter()
                        .find(|(n, _, _)| n == "calc")
                        .map(|(_, v, _)| v.clone());
                    if got.as_deref() != Some(*expect) {
                        return Err(format!("preflight calculated {got:?}, expected {expect:?}"));
                    }
                    let mut h = history();
                    h.execute(&doc, Box::new(SetTextValue::new("trigger", "x")))
                        .map_err(|e| e.to_string())?;
                    let t = FormTree::load(&doc).unwrap();
                    let v = t
                        .by_name("calc")
                        .unwrap()
                        .value
                        .as_text()
                        .unwrap_or("")
                        .to_string();
                    if v != *expect {
                        return Err(format!("stored calc value {v:?}, expected {expect:?}"));
                    }
                    Ok(())
                })()
            }
            _ => {
                counts.library_only += 1;
                Ok(())
            }
        });
        if let Err(e) = r {
            failures.push(format!("{} [{}]: {e}", c.id, c.source));
        }
    }
    eprintln!(
        "AF reference replay: {} cases total; {} format, {} keystroke/validate, {} calculate through the event model; {} library-only",
        cases.len(),
        counts.format,
        counts.key,
        counts.calc,
        counts.library_only
    );
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
    assert!(counts.format + counts.key + counts.calc >= 400);
}

/// The clock override really reaches the format event: a day/month input takes today's year.
#[test]
fn fixed_clock_is_honoured() {
    for (y, m, d) in [(2014, 5, 9), (2031, 12, 24)] {
        let env = papyrine_forms::DateEnv {
            today: papyrine_forms::datefmt::DateTime::ymd(y, m, d),
        };
        let doc = open(one_field("F", "AFDate_FormatEx(\"yyyy-mm-dd\");"));
        let pf = with_fixed_today(env, || preflight_text(&doc, &"f".into(), "3/4").unwrap());
        assert_eq!(pf.display, format!("{y}-03-04"));
    }
}
