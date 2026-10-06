//! Reference cases for the AF functions (ROADMAP 1.13: >= 200 cases).
//!
//! Acrobat itself is not available here, so the expectations come from public
//! references and every case names its source:
//!
//! * `pdfium:<file>:<line>`: PDFium's BSD-3-Clause tests
//!   (`testing/resources/javascript/public_methods.in`,
//!   `fxjs/cjs_publicmethods_embeddertest.cpp`). PDFium reimplements Acrobat's
//!   AF functions and its tests encode Acrobat's observed output (several are
//!   annotated "Acrobat behaves ..."). The Format cases from `public_methods.in`
//!   are generated into `tests/data/pdfium_format_cases.in`.
//! * `adobe:<function>`: Adobe's JavaScript for Acrobat API Reference
//!   (documented semantics: style tables, format tokens). Expected values are
//!   derived by hand from those tables, not captured from a running Acrobat.
//! * `pdfium-src:<function>`: derived by reading `fxjs/cjs_publicmethods.cpp`.
//!
//! A case whose `note` starts with `DISAGREE` is one where our implementation
//! deliberately differs from PDFium (or from what we believe Acrobat does);
//! the note records both values. Those need a run in Acrobat Reader to settle.

use std::collections::HashMap;

use papyrine_forms::datefmt::DateTime;
use papyrine_forms::event::{Alert, Event, TextColor, merge_change};
use papyrine_forms::{DateEnv, MapForm, run_script};

/// `pub` so `papyrine-fill`'s end-to-end test can include this file and replay every case
/// through the event model.
pub struct Case {
    pub id: String,
    pub source: String,
    pub note: &'static str,
    pub kind: Kind,
}

pub enum Kind {
    /// Run a format/calculate script; compare `event.value`.
    Format {
        script: String,
        init: String,
        expect: String,
        color: Option<TextColor>,
    },
    /// Run a script on a keystroke/commit event; compare rc, change and alerts.
    Key {
        script: String,
        value: &'static str,
        change: &'static str,
        sel: (usize, usize),
        commit: bool,
        rc: bool,
        change_out: Option<&'static str>,
        alert: bool,
    },
    /// Calculation over a field set.
    Calc {
        script: String,
        fields: Vec<(&'static str, &'static str)>,
        expect: &'static str,
    },
    Merge {
        value: &'static str,
        change: &'static str,
        sel: (usize, usize),
        commit: bool,
        expect: &'static str,
    },
    MakeNumber {
        input: &'static str,
        expect: f64,
    },
    /// `datefmt::parse_date_ex` then `print_date`.
    ParsePrint {
        value: &'static str,
        parse_fmt: &'static str,
        print_fmt: &'static str,
        expect: Option<&'static str>,
    },
}

pub fn env() -> DateEnv {
    // PDFium's date tests run with the clock fixed at 2014-05-09.
    DateEnv {
        today: DateTime::ymd(2014, 5, 9),
    }
}

fn fmt(id: &str, source: &str, script: &str, init: &str, expect: &str) -> Case {
    Case {
        id: id.into(),
        source: source.into(),
        note: "",
        kind: Kind::Format {
            script: script.into(),
            init: init.into(),
            expect: expect.into(),
            color: None,
        },
    }
}

fn fmt_c(id: &str, source: &str, script: &str, init: &str, expect: &str, color: TextColor) -> Case {
    Case {
        id: id.into(),
        source: source.into(),
        note: "",
        kind: Kind::Format {
            script: script.into(),
            init: init.into(),
            expect: expect.into(),
            color: Some(color),
        },
    }
}

fn dis(mut c: Case, note: &'static str) -> Case {
    c.note = note;
    c
}

#[allow(clippy::too_many_arguments)]
fn key(
    id: &str,
    source: &str,
    script: &str,
    value: &'static str,
    change: &'static str,
    sel: (usize, usize),
    rc: bool,
) -> Case {
    Case {
        id: id.into(),
        source: source.into(),
        note: "",
        kind: Kind::Key {
            script: script.into(),
            value,
            change,
            sel,
            commit: false,
            rc,
            change_out: None,
            alert: false,
        },
    }
}

fn key_out(mut c: Case, out: &'static str) -> Case {
    if let Kind::Key { change_out, .. } = &mut c.kind {
        *change_out = Some(out);
    }
    c
}

fn commit(
    id: &str,
    source: &str,
    script: &str,
    value: &'static str,
    rc: bool,
    alert: bool,
) -> Case {
    Case {
        id: id.into(),
        source: source.into(),
        note: "",
        kind: Kind::Key {
            script: script.into(),
            value,
            change: "",
            sel: (0, 0),
            commit: true,
            rc,
            change_out: None,
            alert,
        },
    }
}

fn calc(
    id: &str,
    source: &str,
    script: &str,
    fields: &[(&'static str, &'static str)],
    expect: &'static str,
) -> Case {
    Case {
        id: id.into(),
        source: source.into(),
        note: "",
        kind: Kind::Calc {
            script: script.into(),
            fields: fields.to_vec(),
            expect,
        },
    }
}

fn pdfium_format_cases(out: &mut Vec<Case>) {
    const ROWS: &[(u32, &str, &str, &str)] = include!("data/pdfium_format_cases.in");
    for &(line, init, expr, expect) in ROWS {
        let id = format!("pdfium-format-{line}");
        let source = format!("pdfium:public_methods.in:{line}");
        // Lines we do not run as-is, with the reason.
        let skip_or_disagree: Option<(bool, &'static str)> = match line {
            // PDFium coerces a numeric argument to a format string; the strict
            // recognizer only accepts string literals there.
            164 | 165 => Some((
                true,
                "AFDate_FormatEx with a non-literal-format argument; outside the grammar",
            )),
            // loose `==` artefacts: `'' == 0` is true in JS, so these pass without computing anything.
            518 | 521 => Some((true, "passes via JS loose equality only")),
            // Extra arguments are ignored by Acrobat; the recognizer rejects them by design.
            237..=239 => Some((
                true,
                "extra arguments rejected by the strict recognizer by design",
            )),
            166 => Some((
                false,
                "DISAGREE: PDFium prints today's date (5/9) for the non-date text \"x\"; we leave the text and raise an invalid-date alert",
            )),
            170 => Some((
                false,
                "DISAGREE: PDFium falls back to today's date (05/09/2014) for 20122015; we reject it",
            )),
            _ => None,
        };
        if let Some((true, _)) = skip_or_disagree {
            continue;
        }
        let mut c = Case {
            id,
            source,
            note: "",
            kind: if expr.starts_with("AFSimple_Calculate") {
                Kind::Calc {
                    script: expr.to_string(),
                    fields: vec![("Text2", "123"), ("Text3", "456"), ("Text4", "407.96")],
                    expect,
                }
            } else {
                Kind::Format {
                    script: expr.to_string(),
                    init: init.to_string(),
                    expect: expect.to_string(),
                    color: None,
                }
            },
        };
        match line {
            166 => {
                c.note = "DISAGREE: PDFium prints today's date (5/9) for the non-date text \"x\"; we keep \"x\" and alert";
                c.kind = Kind::Format {
                    script: expr.into(),
                    init: init.into(),
                    expect: "x".into(),
                    color: None,
                };
            }
            170 => {
                c.note = "DISAGREE: PDFium falls back to today's date (05/09/2014) for 20122015; we reject and keep the text";
                c.kind = Kind::Format {
                    script: expr.into(),
                    init: init.into(),
                    expect: "20122015".into(),
                    color: None,
                };
            }
            _ => {}
        }
        out.push(c);
    }
}

fn number_cases(out: &mut Vec<Case>) {
    let nf = |nd: u8, sep: u8, neg: u8, cur: &str, pre: bool| {
        format!("AFNumber_Format({nd}, {sep}, {neg}, 0, \"{cur}\", {pre});")
    };
    let a = "adobe:AFNumber_Format";
    // separator styles (Adobe: 0 = 1,234.56  1 = 1234.56  2 = 1.234,56  3 = 1234,56)
    for (sep, exp) in [
        (0, "1,234,567.89"),
        (1, "1234567.89"),
        (2, "1.234.567,89"),
        (3, "1234567,89"),
    ] {
        out.push(fmt(
            &format!("num-sep{sep}"),
            a,
            &nf(2, sep, 0, "", false),
            "1234567.891",
            exp,
        ));
    }
    out.push(dis(
        fmt("num-sep4", a, &nf(2, 4, 0, "", false), "1234567.891", "1'234'567.89"),
        "DISAGREE: PDFium AFNumber_Format maps sepStyle 4 to 0 (\"1,234,567.89\"); Acrobat documents 4 as apostrophe grouping and PDFium's own AFPercent_Format honours it",
    ));
    // decimals
    for (nd, exp) in [
        (0, "1235"),
        (1, "1234.6"),
        (2, "1234.57"),
        (3, "1234.568"),
        (4, "1234.5678"),
    ] {
        out.push(fmt(
            &format!("num-dec{nd}"),
            a,
            &nf(nd, 1, 0, "", false),
            "1234.5678",
            exp,
        ));
    }
    // currency, prepend vs append
    out.push(fmt(
        "num-cur-prepend",
        a,
        &nf(2, 0, 0, "$", true),
        "1234.5",
        "$1,234.50",
    ));
    out.push(fmt(
        "num-cur-append",
        a,
        &nf(2, 2, 0, " EUR", false),
        "1234.5",
        "1.234,50 EUR",
    ));
    out.push(fmt(
        "num-cur-pound",
        a,
        &nf(2, 0, 0, "\\u00a3", true),
        "99",
        "\u{a3}99.00",
    ));
    // negative styles: 0 MinusBlack 1 Red 2 ParensBlack 3 ParensRed
    out.push(fmt(
        "num-neg0",
        a,
        &nf(2, 0, 0, "", false),
        "-1234.5",
        "-1,234.50",
    ));
    out.push(dis(
        fmt_c("num-neg1", a, &nf(2, 0, 1, "", false), "-1234.5", "-1,234.50", TextColor::Red),
        "DISAGREE: PDFium inserts no sign for negStyle 1 (\"1,234.50\" in red); Acrobat's UI lists style 1 as the minus form in red",
    ));
    out.push(fmt(
        "num-neg2",
        a,
        &nf(2, 0, 2, "", false),
        "-1234.5",
        "(1,234.50)",
    ));
    out.push(fmt_c(
        "num-neg3",
        a,
        &nf(2, 0, 3, "", false),
        "-1234.5",
        "(1,234.50)",
        TextColor::Red,
    ));
    out.push(fmt_c(
        "num-neg1-pos",
        a,
        &nf(2, 0, 1, "", false),
        "10",
        "10.00",
        TextColor::Black,
    ));
    out.push(fmt_c(
        "num-neg3-pos",
        a,
        &nf(2, 0, 3, "", false),
        "10",
        "10.00",
        TextColor::Black,
    ));
    out.push(fmt(
        "num-neg0-cur",
        "pdfium-src:AFNumber_Format",
        &nf(2, 0, 0, "$", true),
        "-5",
        "-$5.00",
    ));
    out.push(fmt(
        "num-neg2-cur",
        "pdfium-src:AFNumber_Format",
        &nf(2, 0, 2, "$", true),
        "-5",
        "($5.00)",
    ));
    out.push(fmt(
        "num-neg2-cur-append",
        "pdfium-src:AFNumber_Format",
        &nf(0, 0, 2, " kr", false),
        "-1500",
        "(1,500 kr)",
    ));
    // parsing of the raw value
    out.push(fmt(
        "num-empty",
        "pdfium-src:AFNumber_Format",
        &nf(2, 0, 0, "", false),
        "",
        "",
    ));
    out.push(fmt(
        "num-spaces-only",
        "pdfium-src:AFNumber_Format",
        &nf(2, 0, 0, "", false),
        "   ",
        "   ",
    ));
    out.push(fmt(
        "num-garbage",
        "pdfium:public_methods.in:212",
        &nf(2, 0, 0, "", false),
        "abc",
        "0.00",
    ));
    out.push(fmt(
        "num-trailing-garbage",
        "pdfium-src:AFNumber_Format",
        &nf(2, 0, 0, "", false),
        "12abc",
        "12.00",
    ));
    out.push(fmt(
        "num-trim",
        "pdfium-src:AFNumber_Format",
        &nf(1, 1, 0, "", false),
        "  7.25  ",
        "7.3",
    ));
    out.push(fmt(
        "num-decimal-comma-input",
        "pdfium-src:AFNumber_Format",
        &nf(1, 1, 0, "", false),
        "12,5",
        "12.5",
    ));
    out.push(fmt(
        "num-zero",
        "adobe:AFNumber_Format",
        &nf(2, 0, 0, "", false),
        "0",
        "0.00",
    ));
    out.push(fmt(
        "num-negzero",
        "pdfium-src:AFNumber_Format",
        &nf(2, 0, 0, "", false),
        "-0",
        "0.00",
    ));
    out.push(fmt(
        "num-small-neg",
        "pdfium-src:AFNumber_Format",
        &nf(2, 0, 0, "", false),
        "-0.001",
        "-0.00",
    ));
    out.push(fmt(
        "num-int-sep",
        "adobe:AFNumber_Format",
        &nf(0, 0, 0, "", false),
        "1000",
        "1,000",
    ));
    out.push(fmt(
        "num-999",
        "adobe:AFNumber_Format",
        &nf(0, 0, 0, "", false),
        "999",
        "999",
    ));
    out.push(fmt(
        "num-1e6",
        "adobe:AFNumber_Format",
        &nf(0, 0, 0, "", false),
        "1000000",
        "1,000,000",
    ));
    out.push(fmt(
        "num-exp",
        "pdfium-src:AFNumber_Format",
        &nf(0, 1, 0, "", false),
        "1e3",
        "1000",
    ));
    out.push(fmt(
        "num-plus",
        "pdfium-src:AFNumber_Format",
        &nf(1, 1, 0, "", false),
        "+3",
        "3.0",
    ));
    // PDFium adds 1e-15 before rounding so decimal ties round up the way users expect.
    out.push(fmt(
        "num-round-0.005",
        "pdfium-src:AFNumber_Format",
        &nf(2, 1, 0, "", false),
        "0.005",
        "0.01",
    ));
    out.push(fmt(
        "num-round-1.005",
        "pdfium-src:AFNumber_Format",
        &nf(2, 1, 0, "", false),
        "1.005",
        "1.01",
    ));
    out.push(fmt(
        "num-round-2.675",
        "pdfium-src:AFNumber_Format",
        &nf(2, 1, 0, "", false),
        "2.675",
        "2.68",
    ));
    out.push(fmt(
        "num-nneg-dec",
        "pdfium-src:AFNumber_Format",
        &nf(2, 1, 0, "", false),
        "3",
        "3.00",
    ));
    out.push(fmt(
        "num-12-0",
        "pdfium:public_methods.in:213",
        &nf(0, 1, 0, "", false),
        "12",
        "12",
    ));
    out.push(fmt(
        "num-dec-clamp",
        "pdfium-src:AFNumber_Format",
        "AFNumber_Format(20, 1, 0, 0, \"\", false);",
        "1.5",
        "1.500000000000001",
    ));
    out.push(fmt(
        "num-sep2-small",
        "adobe:AFNumber_Format",
        &nf(2, 2, 0, "", false),
        "0.5",
        "0,50",
    ));
    out.push(fmt(
        "num-sep3-big",
        "adobe:AFNumber_Format",
        &nf(2, 3, 0, "", false),
        "98765.4321",
        "98765,43",
    ));
    out.push(fmt(
        "num-5-args",
        "adobe:AFNumber_Format",
        "AFNumber_Format(2, 0, 0, 0, \"$\");",
        "5",
        "5.00$",
    ));
}

fn percent_extra(out: &mut Vec<Case>) {
    let a = "adobe:AFPercent_Format";
    out.push(fmt("pct-0.5", a, "AFPercent_Format(0, 0);", "0.5", "50%"));
    out.push(fmt("pct-1", a, "AFPercent_Format(0, 0);", "1", "100%"));
    out.push(fmt(
        "pct-prepend",
        a,
        "AFPercent_Format(1, 0, true);",
        "0.256",
        "%25.6",
    ));
    out.push(fmt(
        "pct-sep2",
        a,
        "AFPercent_Format(2, 2);",
        "0.1234",
        "12,34%",
    ));
    out.push(fmt(
        "pct-neg-grouped",
        "pdfium:public_methods.in:244",
        "AFPercent_Format(0, 0);",
        "-12.3456",
        "-1,235%",
    ));
    out.push(fmt(
        "pct-text",
        "pdfium-src:AFPercent_Format",
        "AFPercent_Format(1, 0);",
        "abc",
        "0.0%",
    ));
}

fn date_cases(out: &mut Vec<Case>) {
    // AFDate_Format(index): the 14 documented patterns (adobe:AFDate_Format table).
    let a = "adobe:AFDate_Format";
    let d = [
        (0, "3/7", "3/7"),
        (1, "3/7/23", "3/7/23"),
        (2, "3/7/23", "03/07/23"),
        (3, "03/23", "03/23"),
        (4, "7-Mar", "7-Mar"),
        (5, "7-Mar-23", "7-Mar-23"),
        (6, "7-Mar-23", "07-Mar-23"),
        (7, "23-03-07", "23-03-07"),
        (8, "Mar-23", "Mar-23"),
        (9, "March-23", "March-23"),
        (10, "Mar 7, 2023", "Mar 7, 2023"),
        (11, "March 7, 2023", "March 7, 2023"),
        (12, "3/7/23 4:05 pm", "3/7/23 4:05 pm"),
        (13, "3/7/23 16:05", "3/7/23 16:05"),
    ];
    for (i, init, exp) in d {
        out.push(fmt(
            &format!("date-idx{i}"),
            a,
            &format!("AFDate_Format({i});"),
            init,
            exp,
        ));
    }
    // out-of-range index falls back to 0 (pdfium-src: WithinBoundsOrZero)
    out.push(fmt(
        "date-idx99",
        "pdfium-src:AFDate_Format",
        "AFDate_Format(99);",
        "3/7",
        "3/7",
    ));
    // reformatting a value typed in another common shape
    let f = "adobe:AFDate_FormatEx";
    out.push(fmt(
        "date-ex-mmddyyyy",
        f,
        "AFDate_FormatEx(\"mm/dd/yyyy\");",
        "03/07/2023",
        "03/07/2023",
    ));
    out.push(fmt(
        "date-ex-single-digits",
        f,
        "AFDate_FormatEx(\"mm/dd/yyyy\");",
        "3/7/2023",
        "03/07/2023",
    ));
    out.push(fmt(
        "date-ex-dd-mmm-yyyy",
        f,
        "AFDate_FormatEx(\"dd-mmm-yyyy\");",
        "07-Mar-2023",
        "07-Mar-2023",
    ));
    out.push(fmt(
        "date-ex-mmmm",
        f,
        "AFDate_FormatEx(\"mmmm d, yyyy\");",
        "March 7, 2023",
        "March 7, 2023",
    ));
    out.push(fmt(
        "date-ex-compact",
        f,
        "AFDate_FormatEx(\"mmddyyyy\");",
        "03072023",
        "03072023",
    ));
    out.push(fmt(
        "date-ex-ddmmyyyy",
        f,
        "AFDate_FormatEx(\"ddmmyyyy\");",
        "31122023",
        "31122023",
    ));
    out.push(fmt(
        "date-ex-yyyy-mm-dd",
        f,
        "AFDate_FormatEx(\"yyyy-mm-dd\");",
        "2023-03-07",
        "2023-03-07",
    ));
    out.push(fmt(
        "date-ex-dots",
        f,
        "AFDate_FormatEx(\"dd.mm.yyyy\");",
        "07.03.2023",
        "07.03.2023",
    ));
    out.push(fmt(
        "date-ex-weekday",
        f,
        "AFDate_FormatEx(\"dddd, mmmm d, yyyy\");",
        "Tuesday, March 7, 2023",
        "Tuesday, March 7, 2023",
    ));
    out.push(fmt(
        "date-ex-empty",
        f,
        "AFDate_FormatEx(\"mm/dd/yyyy\");",
        "",
        "",
    ));
    out.push(fmt(
        "date-ex-leap",
        f,
        "AFDate_FormatEx(\"mm/dd/yyyy\");",
        "02/29/2024",
        "02/29/2024",
    ));
    out.push(dis(
        fmt("date-ex-not-leap", f, "AFDate_FormatEx(\"mm/dd/yyyy\");", "02/29/2023", "02/29/2023"),
        "DISAGREE (unverified): PDFium's MakeDay rolls Feb 29 2023 over to 03/01/2023; Acrobat's alert says the date must exist, so we reject and keep the text",
    ));
    out.push(fmt(
        "date-ex-garbage",
        f,
        "AFDate_FormatEx(\"mm/dd/yyyy\");",
        "hello",
        "hello",
    ));
    out.push(fmt(
        "date-ex-pdfium-mmddyyyy-adjacent",
        "pdfium:public_methods.in:169",
        "AFDate_FormatEx(\"mm/dd/yyyy\");",
        "12302015",
        "12/02/2015",
    ));
    // 2-digit year pivot: < 50 is 20xx, otherwise 19xx (pdfium-src:FX_ParseDateUsingFormat comment)
    out.push(fmt(
        "date-yy-pivot-49",
        "pdfium-src:FX_ParseDateUsingFormat",
        "AFDate_FormatEx(\"mm/dd/yyyy\");",
        "01/02/49",
        "01/02/2049",
    ));
    out.push(fmt(
        "date-yy-pivot-50",
        "pdfium-src:FX_ParseDateUsingFormat",
        "AFDate_FormatEx(\"mm/dd/yyyy\");",
        "01/02/50",
        "01/02/1950",
    ));
    // unambiguous day/month swap in the free-form fallback
    out.push(fmt(
        "date-swap-dmy",
        "pdfium-src:ParseDate",
        "AFDate_FormatEx(\"mm/dd/yyyy\");",
        "25/12/2023",
        "12/25/2023",
    ));
    // time formats
    let t = "adobe:AFTime_Format";
    for (i, init, exp) in [
        (0, "16:05", "16:05"),
        (1, "16:05", "4:05 pm"),
        (2, "9:07:03", "09:07:03"),
        (3, "21:07:03", "9:07:03 pm"),
        (0, "7:05", "07:05"),
        (1, "9:30 am", "9:30 am"),
        (3, "9:30:15 pm", "9:30:15 pm"),
    ] {
        out.push(fmt(
            &format!("time-idx{i}-{init}"),
            t,
            &format!("AFTime_Format({i});"),
            init,
            exp,
        ));
    }
    out.push(fmt(
        "time-ex",
        "adobe:AFTime_FormatEx",
        "AFTime_FormatEx(\"HH:MM\");",
        "6:09",
        "06:09",
    ));
    out.push(fmt(
        "time-ex-tt",
        "adobe:AFTime_FormatEx",
        "AFTime_FormatEx(\"hh:MM tt\");",
        "17:45",
        "05:45 pm",
    ));
    out.push(dis(
        fmt("time-midnight-12h", "adobe:util.printd", "AFTime_Format(1);", "00:05", "12:05 am"),
        "DISAGREE: PDFium's PrintDateUsingFormat prints h as 0 for hour 0 (\"0:05 am\"); the 12-hour clock has no hour 0 in Acrobat",
    ));
    out.push(dis(
        fmt("time-noon-12h", "adobe:util.printd", "AFTime_Format(1);", "12:00", "12:00 pm"),
        "DISAGREE: PDFium's PrintDateUsingFormat selects am for hour 12 (hour > 12 test); noon is pm",
    ));
    out.push(fmt(
        "time-invalid-hour",
        "pdfium-src:FX_IsValid24Hour",
        "AFTime_Format(0);",
        "25:00",
        "25:00",
    ));
    out.push(fmt(
        "time-24h-ok",
        "pdfium-src:FX_IsValid24Hour",
        "AFTime_Format(0);",
        "24:00",
        "24:00",
    ));
}

fn parse_print_cases(out: &mut Vec<Case>) {
    let p = |id: &str,
             source: &str,
             value: &'static str,
             pf: &'static str,
             of: &'static str,
             expect: Option<&'static str>| Case {
        id: id.into(),
        source: source.into(),
        note: "",
        kind: Kind::ParsePrint {
            value,
            parse_fmt: pf,
            print_fmt: of,
            expect,
        },
    };
    let s = "pdfium:cjs_publicmethods_embeddertest.cpp:ParseDateUsingFormat";
    // value, parse format -> printed as yyyy-mm-dd
    out.push(p(
        "parse-1968",
        s,
        "06/25/1968",
        "mm/dd/yyyy",
        "yyyy-mm-dd",
        Some("1968-06-25"),
    ));
    out.push(p(
        "parse-1968-ddmmyyyy",
        s,
        "25061968",
        "ddmmyyyy",
        "yyyy-mm-dd",
        Some("1968-06-25"),
    ));
    out.push(p(
        "parse-1968-yyyymmdd",
        s,
        "19680625",
        "yyyymmdd",
        "yyyy-mm-dd",
        Some("1968-06-25"),
    ));
    out.push(p(
        "parse-1985",
        s,
        "31121985",
        "ddmmyyyy",
        "yyyy-mm-dd",
        Some("1985-12-31"),
    ));
    out.push(p(
        "parse-1985-yy",
        s,
        "311285",
        "ddmmyy",
        "yyyy-mm-dd",
        Some("1985-12-31"),
    ));
    out.push(p(
        "parse-1995",
        s,
        "01021995",
        "ddmmyyyy",
        "yyyy-mm-dd",
        Some("1995-02-01"),
    ));
    out.push(p(
        "parse-1995-yy",
        s,
        "010295",
        "ddmmyy",
        "yyyy-mm-dd",
        Some("1995-02-01"),
    ));
    out.push(p(
        "parse-2005",
        s,
        "01022005",
        "ddmmyyyy",
        "yyyy-mm-dd",
        Some("2005-02-01"),
    ));
    out.push(p(
        "parse-2005-yy",
        s,
        "010205",
        "ddmmyy",
        "yyyy-mm-dd",
        Some("2005-02-01"),
    ));
    out.push(p(
        "parse-2005-yymmdd",
        s,
        "050201",
        "yymmdd",
        "yyyy-mm-dd",
        Some("2005-02-01"),
    ));
    // PrintDateUsingFormat: parse a known date and print with each pdfium format.
    let q = "pdfium:cjs_publicmethods_embeddertest.cpp:PrintDateUsingFormat";
    out.push(p(
        "print-ddmmyy-1968",
        q,
        "1968-06-25",
        "yyyy-mm-dd",
        "ddmmyy",
        Some("250668"),
    ));
    out.push(p(
        "print-yy/mm/dd-1968",
        q,
        "1968-06-25",
        "yyyy-mm-dd",
        "yy/mm/dd",
        Some("68/06/25"),
    ));
    out.push(p(
        "print-ddmmyy-1969",
        q,
        "1969-12-31",
        "yyyy-mm-dd",
        "ddmmyy",
        Some("311269"),
    ));
    out.push(p(
        "print-yy!mmdd-1969",
        q,
        "1969-12-31",
        "yyyy-mm-dd",
        "yy!mmdd",
        Some("69!1231"),
    ));
    out.push(p(
        "print-ddmmyy-1970",
        q,
        "1970-01-01",
        "yyyy-mm-dd",
        "ddmmyy",
        Some("010170"),
    ));
    out.push(p(
        "print-mm-yyyy-dd-1970",
        q,
        "1970-01-01",
        "yyyy-mm-dd",
        "mm-yyyy-dd",
        Some("01-1970-01"),
    ));
    out.push(p(
        "print-yymmdd-1985",
        q,
        "1985-12-31",
        "yyyy-mm-dd",
        "yymmdd",
        Some("851231"),
    ));
    out.push(p(
        "print-yyyymmdd-1995",
        q,
        "1995-02-01",
        "yyyy-mm-dd",
        "yyyymmdd",
        Some("19950201"),
    ));
    out.push(p(
        "print-yyyyddmm-2005",
        q,
        "2005-02-01",
        "yyyy-mm-dd",
        "yyyyddmm",
        Some("20050102"),
    ));
    out.push(p(
        "print-yyyydd-2085",
        q,
        "2085-12-31",
        "yyyy-mm-dd",
        "yyyydd",
        Some("208531"),
    ));
    out.push(p(
        "print-mmddyyyy-2095",
        q,
        "2095-02-01",
        "yyyy-mm-dd",
        "mmddyyyy",
        Some("02012095"),
    ));
    // util.printd token table (adobe:util.printd), 2014-07-04 is a Friday
    let u = "adobe:util.printd";
    for (f, exp) in [
        ("mmmm", "July"),
        ("mmm", "Jul"),
        ("mm", "07"),
        ("m", "7"),
        ("dddd", "Friday"),
        ("ddd", "Fri"),
        ("dd", "04"),
        ("d", "4"),
        ("yyyy", "2014"),
        ("yy", "14"),
        ("yyyy-mm-dd", "2014-07-04"),
    ] {
        out.push(p(
            &format!("printd-{f}"),
            u,
            "2014-07-04",
            "yyyy-mm-dd",
            f,
            Some(exp),
        ));
    }
    out.push(p(
        "parse-month-name-fallback",
        "pdfium-src:ParseDateUsingFormat (Date.parse fallback)",
        "Jul 4, 2014",
        "mm/dd/yyyy",
        "yyyy-mm-dd",
        Some("2014-07-04"),
    ));
    out.push(p(
        "parse-iso-fallback",
        "pdfium-src:ParseDateUsingFormat (Date.parse fallback)",
        "2014-07-04",
        "mm/dd/yyyy",
        "yyyy-mm-dd",
        Some("2014-07-04"),
    ));
    out.push(p(
        "parse-impossible-day",
        "adobe:AFDate_KeystrokeEx",
        "04/31/2014",
        "mm/dd/yyyy",
        "yyyy-mm-dd",
        None,
    ));
    out.push(p(
        "parse-impossible-month",
        "adobe:AFDate_KeystrokeEx",
        "99/99/9999",
        "mm/dd/yyyy",
        "yyyy-mm-dd",
        None,
    ));
    out.push(p(
        "parse-text",
        "adobe:AFDate_KeystrokeEx",
        "tomorrow",
        "mm/dd/yyyy",
        "yyyy-mm-dd",
        None,
    ));
}

fn special_cases(out: &mut Vec<Case>) {
    let a = "adobe:AFSpecial_Format";
    out.push(fmt(
        "special-zip",
        a,
        "AFSpecial_Format(0);",
        "12345",
        "12345",
    ));
    out.push(fmt(
        "special-zip-truncates",
        a,
        "AFSpecial_Format(0);",
        "123456789",
        "12345",
    ));
    out.push(fmt(
        "special-zip4",
        a,
        "AFSpecial_Format(1);",
        "123456789",
        "12345-6789",
    ));
    out.push(fmt(
        "special-zip4-already",
        a,
        "AFSpecial_Format(1);",
        "12345-6789",
        "12345-6789",
    ));
    out.push(fmt(
        "special-phone10",
        a,
        "AFSpecial_Format(2);",
        "5551234567",
        "(555) 123-4567",
    ));
    out.push(fmt(
        "special-phone7",
        a,
        "AFSpecial_Format(2);",
        "5551234",
        "555-1234",
    ));
    out.push(fmt(
        "special-phone-punct",
        a,
        "AFSpecial_Format(2);",
        "(555) 123-4567",
        "(555) 123-4567",
    ));
    out.push(fmt(
        "special-phone-dashes",
        a,
        "AFSpecial_Format(2);",
        "555-123-4567",
        "(555) 123-4567",
    ));
    out.push(fmt(
        "special-ssn",
        a,
        "AFSpecial_Format(3);",
        "123456789",
        "123-45-6789",
    ));
    out.push(fmt(
        "special-ssn-already",
        a,
        "AFSpecial_Format(3);",
        "123-45-6789",
        "123-45-6789",
    ));
    out.push(fmt(
        "special-ssn-short",
        a,
        "AFSpecial_Format(3);",
        "12345",
        "123-45-",
    ));
    out.push(fmt(
        "special-ssn-letters",
        a,
        "AFSpecial_Format(3);",
        "12a34b5678",
        "123-45-678",
    ));
    out.push(fmt(
        "special-bad-index",
        "pdfium-src:AFSpecial_Format",
        "AFSpecial_Format(9);",
        "12345",
        "",
    ));
    out.push(fmt(
        "special-zip-pdfium-1",
        "pdfium:public_methods.in:544",
        "AFSpecial_Format(0);",
        "0123456789",
        "01234",
    ));
}

fn keystroke_cases(out: &mut Vec<Case>) {
    let nk = "AFNumber_Keystroke(2, 0, 0, 0, \"\", true);";
    let nk2 = "AFNumber_Keystroke(2, 2, 0, 0, \"\", true);";
    let a = "pdfium-src:AFNumber_Keystroke";
    out.push(key("nks-digit", a, nk, "12", "3", (2, 2), true));
    out.push(key("nks-letter", a, nk, "12", "a", (2, 2), false));
    out.push(key("nks-space", a, nk, "12", " ", (2, 2), false));
    out.push(key("nks-dot", a, nk, "12", ".", (2, 2), true));
    out.push(key("nks-second-dot", a, nk, "12.5", ".", (4, 4), false));
    out.push(key(
        "nks-comma-rejected-sep0",
        a,
        nk,
        "12",
        ",",
        (2, 2),
        false,
    ));
    out.push(key("nks-minus-start", a, nk, "12", "-", (0, 0), true));
    out.push(key("nks-minus-middle", a, nk, "12", "-", (1, 1), false));
    out.push(key("nks-minus-twice", a, nk, "-12", "-", (0, 0), false));
    out.push(key("nks-paste-digits", a, nk, "", "123456", (0, 0), true));
    out.push(key("nks-paste-bad", a, nk, "", "12a", (0, 0), false));
    out.push(key("nks-paste-two-dots", a, nk, "", "1.2.3", (0, 0), false));
    out.push(key(
        "nks-replace-sign-selection",
        a,
        nk,
        "-12",
        "5",
        (0, 1),
        true,
    ));
    out.push(key(
        "nks-selection-before-sign",
        a,
        nk,
        "1-2",
        "5",
        (0, 1),
        false,
    ));
    out.push(key("nks-sep2-comma", a, nk2, "12", ",", (2, 2), true));
    out.push(key(
        "nks-sep2-dot-rejected",
        a,
        nk2,
        "12",
        ".",
        (2, 2),
        false,
    ));
    out.push(key(
        "nks-empty-change",
        "pdfium:cjs_publicmethods_embeddertest.cpp:AFNumberKeystroke",
        nk,
        "-10",
        "",
        (0, 0),
        true,
    ));
    out.push(key(
        "nks-pct",
        "pdfium-src:AFPercent_Keystroke",
        "AFPercent_Keystroke(2, 0);",
        "1",
        "5",
        (1, 1),
        true,
    ));
    out.push(key(
        "nks-pct-letter",
        "pdfium-src:AFPercent_Keystroke",
        "AFPercent_Keystroke(2, 0);",
        "1",
        "x",
        (1, 1),
        false,
    ));
    let c = "pdfium-src:AFNumber_Keystroke";
    out.push(commit("nkc-valid", c, nk, "123.45", true, false));
    out.push(commit("nkc-comma", c, nk, "1,5", true, false));
    out.push(commit("nkc-negative", c, nk, "-12", true, false));
    out.push(commit("nkc-empty", c, nk, "", true, false));
    out.push(commit(
        "nkc-garbage",
        "pdfium:public_methods.in:218",
        nk,
        "abc",
        false,
        true,
    ));
    out.push(commit("nkc-two-dots", c, nk, "1.2.3", false, true));
    out.push(commit("nkc-exponent", c, nk, "1e+3", true, false));
    out.push(commit("nkc-sign-late", c, nk, "1-2", false, true));
    out.push(commit(
        "pkc-garbage",
        "pdfium:public_methods.in:261",
        "AFPercent_Keystroke(1, 0);",
        "abc",
        false,
        true,
    ));
    out.push(commit(
        "pkc-leading-dot",
        "pdfium:public_methods.in:262",
        "AFPercent_Keystroke(1, 0);",
        ".123",
        true,
        false,
    ));
    // special keystrokes
    let s = "pdfium-src:AFSpecial_KeystrokeEx";
    out.push(key(
        "sks-digit-ok",
        s,
        "AFSpecial_KeystrokeEx(\"99999\");",
        "12",
        "3",
        (2, 2),
        true,
    ));
    out.push(key(
        "sks-digit-letter",
        s,
        "AFSpecial_KeystrokeEx(\"99999\");",
        "12",
        "x",
        (2, 2),
        false,
    ));
    out.push(key(
        "sks-too-long",
        s,
        "AFSpecial_KeystrokeEx(\"99999\");",
        "12345",
        "6",
        (5, 5),
        false,
    ));
    out.push(key(
        "sks-A-letter",
        s,
        "AFSpecial_KeystrokeEx(\"AA999\");",
        "",
        "Q",
        (0, 0),
        true,
    ));
    out.push(key(
        "sks-A-digit",
        s,
        "AFSpecial_KeystrokeEx(\"AA999\");",
        "",
        "7",
        (0, 0),
        false,
    ));
    out.push(key(
        "sks-O-alnum",
        s,
        "AFSpecial_KeystrokeEx(\"OO\");",
        "a",
        "7",
        (1, 1),
        true,
    ));
    out.push(key(
        "sks-O-punct",
        s,
        "AFSpecial_KeystrokeEx(\"OO\");",
        "a",
        "-",
        (1, 1),
        false,
    ));
    out.push(key(
        "sks-X-any",
        s,
        "AFSpecial_KeystrokeEx(\"XX\");",
        "a",
        "-",
        (1, 1),
        true,
    ));
    out.push(key(
        "sks-literal-slot",
        s,
        "AFSpecial_KeystrokeEx(\"999-9999\");",
        "555",
        "1",
        (3, 3),
        true,
    ));
    out.push(key_out(
        key(
            "sks-literal-slot-fills-dash",
            s,
            "AFSpecial_KeystrokeEx(\"999-9999\");",
            "555",
            "1",
            (3, 3),
            true,
        ),
        "-",
    ));
    out.push(key(
        "sks-literal-typed-dash",
        s,
        "AFSpecial_KeystrokeEx(\"999-9999\");",
        "555",
        "-",
        (3, 3),
        true,
    ));
    out.push(key(
        "sks-empty-mask",
        "pdfium:public_methods.in:625",
        "AFSpecial_KeystrokeEx(\"\");",
        "12345",
        "x",
        (5, 5),
        true,
    ));
    out.push(key(
        "sks-phone-typing",
        "pdfium-src:AFSpecial_Keystroke",
        "AFSpecial_Keystroke(2);",
        "555",
        "1",
        (3, 3),
        true,
    ));
    out.push(key(
        "sks-phone-letter",
        "pdfium-src:AFSpecial_Keystroke",
        "AFSpecial_Keystroke(2);",
        "555",
        "x",
        (3, 3),
        false,
    ));
    out.push(key(
        "sks-ssn-typing",
        "pdfium-src:AFSpecial_Keystroke",
        "AFSpecial_Keystroke(3);",
        "12345678",
        "9",
        (8, 8),
        true,
    ));
    out.push(key(
        "sks-ssn-too-long",
        "pdfium-src:AFSpecial_Keystroke",
        "AFSpecial_Keystroke(3);",
        "123456789",
        "0",
        (9, 9),
        false,
    ));
    out.push(key(
        "sks-zip-too-long",
        "pdfium-src:AFSpecial_Keystroke",
        "AFSpecial_Keystroke(0);",
        "12345",
        "6",
        (5, 5),
        false,
    ));
    out.push(key(
        "sks-phone-long",
        "pdfium-src:AFSpecial_Keystroke",
        "AFSpecial_Keystroke(2);",
        "5551234",
        "5",
        (7, 7),
        true,
    ));
    out.push(key(
        "sks-phone-too-long",
        "pdfium-src:AFSpecial_Keystroke",
        "AFSpecial_Keystroke(2);",
        "5551234567",
        "8",
        (10, 10),
        false,
    ));
    // special commits (will-commit events), pdfium public_methods.in test 2
    let t = "pdfium:public_methods.in";
    out.push(commit(
        "skc-empty-mask",
        &format!("{t}:622"),
        "AFSpecial_KeystrokeEx(\"\");",
        "12345",
        true,
        false,
    ));
    out.push(commit(
        "skc-short",
        &format!("{t}:623"),
        "AFSpecial_KeystrokeEx(\"9999\");",
        "123",
        false,
        true,
    ));
    out.push(commit(
        "skc-long",
        &format!("{t}:624"),
        "AFSpecial_KeystrokeEx(\"9999\");",
        "12345",
        false,
        true,
    ));
    out.push(commit(
        "skc-letters",
        &format!("{t}:625"),
        "AFSpecial_KeystrokeEx(\"9999\");",
        "abcd",
        false,
        true,
    ));
    out.push(commit(
        "skc-ok",
        &format!("{t}:626"),
        "AFSpecial_KeystrokeEx(\"9999\");",
        "1234",
        true,
        false,
    ));
    out.push(commit(
        "skc-X",
        &format!("{t}:627"),
        "AFSpecial_KeystrokeEx(\"XXXX\");",
        "abcd",
        true,
        false,
    ));
    out.push(commit(
        "skc-empty-value",
        "pdfium-src:AFSpecial_KeystrokeEx",
        "AFSpecial_KeystrokeEx(\"9999\");",
        "",
        true,
        false,
    ));
    out.push(commit(
        "skc-literal-mask-ok",
        "pdfium-src:AFSpecial_KeystrokeEx",
        "AFSpecial_KeystrokeEx(\"999-9999\");",
        "555-1234",
        true,
        false,
    ));
    out.push(commit(
        "skc-literal-mask-bad",
        "pdfium-src:AFSpecial_KeystrokeEx",
        "AFSpecial_KeystrokeEx(\"999-9999\");",
        "5551234",
        false,
        true,
    ));
    // date keystrokes
    let d = "mm/dd/yyyy";
    let dk = format!("AFDate_KeystrokeEx(\"{d}\");");
    let da = "adobe:AFDate_KeystrokeEx";
    out.push(commit("dkc-valid", da, &dk, "03/07/2023", true, false));
    out.push(commit(
        "dkc-empty",
        "pdfium-src:AFDate_KeystrokeEx",
        &dk,
        "",
        true,
        false,
    ));
    out.push(commit("dkc-garbage", da, &dk, "hello", false, true));
    out.push(commit("dkc-feb30", da, &dk, "02/30/2023", false, true));
    out.push(commit("dkc-leap", da, &dk, "02/29/2024", true, false));
    out.push(commit("dkc-notleap", da, &dk, "02/29/2023", false, true));
    out.push(commit(
        "dkc-month13-swaps",
        "pdfium-src:ParseDate",
        &dk,
        "13/01/2023",
        true,
        false,
    ));
    out.push(commit("dkc-day32", da, &dk, "01/32/2023", false, true));
    out.push(commit(
        "dkc-day-name-fallback",
        "pdfium-src:ParseDateUsingFormat",
        &dk,
        "Mar 7, 2023",
        true,
        false,
    ));
    out.push(commit(
        "dkc-idx-1",
        "adobe:AFDate_Keystroke",
        "AFDate_Keystroke(2);",
        "3/7/23",
        true,
        false,
    ));
    out.push(commit(
        "dkc-idx-bad",
        "adobe:AFDate_Keystroke",
        "AFDate_Keystroke(2);",
        "zz",
        false,
        true,
    ));
    out.push(commit(
        "dkc-pdfium-partial",
        "pdfium:public_methods.in:176",
        "AFDate_Keystroke(2);",
        "04/19",
        true,
        false,
    ));
    out.push(commit(
        "tkc-valid",
        "adobe:AFTime_Keystroke",
        "AFTime_Keystroke(0);",
        "16:05",
        true,
        false,
    ));
    out.push(commit(
        "tkc-pm",
        "adobe:AFTime_Keystroke",
        "AFTime_Keystroke(1);",
        "4:05 pm",
        true,
        false,
    ));
    out.push(commit(
        "tkc-bad",
        "adobe:AFTime_Keystroke",
        "AFTime_Keystroke(0);",
        "hello",
        false,
        true,
    ));
    out.push(commit(
        "tkc-minute-61",
        "pdfium-src:FX_IsValidMinute",
        "AFTime_Keystroke(0);",
        "10:61",
        false,
        true,
    ));
    out.push(commit(
        "tkc-ex",
        "adobe:AFTime_KeystrokeEx",
        "AFTime_KeystrokeEx(\"HH:MM:ss\");",
        "10:11:12",
        true,
        false,
    ));
    out.push(commit(
        "tkc-ex-bad",
        "adobe:AFTime_KeystrokeEx",
        "AFTime_KeystrokeEx(\"HH:MM:ss\");",
        "ab:cd:ef",
        false,
        true,
    ));
    // range validation (pdfium public_methods.in comments give the alert kind)
    let r = "AFRange_Validate(true, 2, true, 4);";
    out.push(commit(
        "range-between-low",
        &format!("{t}:603"),
        r,
        "1",
        false,
        true,
    ));
    out.push(commit(
        "range-between-high",
        &format!("{t}:604"),
        r,
        "5",
        false,
        true,
    ));
    out.push(commit(
        "range-between-ok",
        &format!("{t}:608"),
        r,
        "3",
        true,
        false,
    ));
    out.push(commit(
        "range-between-edge-lo",
        "pdfium-src:AFRange_Validate",
        r,
        "2",
        true,
        false,
    ));
    out.push(commit(
        "range-between-edge-hi",
        "pdfium-src:AFRange_Validate",
        r,
        "4",
        true,
        false,
    ));
    out.push(commit(
        "range-ge-low",
        &format!("{t}:605"),
        "AFRange_Validate(true, 2, false, 4);",
        "1",
        false,
        true,
    ));
    out.push(commit(
        "range-le-high",
        &format!("{t}:606"),
        "AFRange_Validate(false, 2, true, 4);",
        "5",
        false,
        true,
    ));
    out.push(commit(
        "range-only-le-ok",
        &format!("{t}:609"),
        "AFRange_Validate(false, 2, true, 4);",
        "1",
        true,
        false,
    ));
    out.push(commit(
        "range-only-ge-ok",
        &format!("{t}:610"),
        "AFRange_Validate(true, 2, false, 4);",
        "5",
        true,
        false,
    ));
    out.push(commit(
        "range-empty",
        "pdfium-src:AFRange_Validate",
        r,
        "",
        true,
        false,
    ));
    out.push(commit(
        "range-negative",
        "adobe:AFRange_Validate",
        "AFRange_Validate(true, -5, true, 5);",
        "-3",
        true,
        false,
    ));
    out.push(commit(
        "range-negative-out",
        "adobe:AFRange_Validate",
        "AFRange_Validate(true, -5, true, 5);",
        "-6",
        false,
        true,
    ));
    out.push(commit(
        "range-decimal",
        "adobe:AFRange_Validate",
        "AFRange_Validate(true, 0, true, 1);",
        "0.5",
        true,
        false,
    ));
    out.push(commit(
        "range-none",
        "pdfium-src:AFRange_Validate",
        "AFRange_Validate(false, 0, false, 0);",
        "99",
        true,
        false,
    ));
    out.push(commit(
        "range-text-is-zero",
        "pdfium-src:AFRange_Validate (atof)",
        "AFRange_Validate(true, 1, false, 0);",
        "abc",
        false,
        true,
    ));
}

fn calc_cases(out: &mut Vec<Case>) {
    let f = [("a", "10"), ("b", "20.5"), ("c", "3"), ("e", "")];
    let s = "adobe:AFSimple_Calculate";
    out.push(calc(
        "calc-sum",
        s,
        "AFSimple_Calculate(\"SUM\", new Array(\"a\", \"b\", \"c\"));",
        &f,
        "33.5",
    ));
    out.push(calc(
        "calc-avg",
        s,
        "AFSimple_Calculate(\"AVG\", new Array(\"a\", \"b\"));",
        &f,
        "15.25",
    ));
    out.push(calc(
        "calc-prd",
        s,
        "AFSimple_Calculate(\"PRD\", new Array(\"a\", \"c\"));",
        &f,
        "30",
    ));
    out.push(calc(
        "calc-min",
        s,
        "AFSimple_Calculate(\"MIN\", new Array(\"a\", \"b\", \"c\"));",
        &f,
        "3",
    ));
    out.push(calc(
        "calc-max",
        s,
        "AFSimple_Calculate(\"MAX\", new Array(\"a\", \"b\", \"c\"));",
        &f,
        "20.5",
    ));
    out.push(calc(
        "calc-string-list",
        s,
        "AFSimple_Calculate(\"SUM\", \"a, b\");",
        &f,
        "30.5",
    ));
    out.push(calc(
        "calc-array-literal",
        s,
        "AFSimple_Calculate(\"SUM\", [\"a\", \"c\"]);",
        &f,
        "13",
    ));
    out.push(calc(
        "calc-empty-field-counts-zero",
        s,
        "AFSimple_Calculate(\"AVG\", new Array(\"a\", \"e\"));",
        &f,
        "5",
    ));
    out.push(calc(
        "calc-missing-field-ignored",
        s,
        "AFSimple_Calculate(\"SUM\", new Array(\"a\", \"zzz\"));",
        &f,
        "10",
    ));
    out.push(calc(
        "calc-no-fields",
        "pdfium-src:AFSimple_Calculate",
        "AFSimple_Calculate(\"SUM\", new Array(\"zzz\"));",
        &f,
        "0",
    ));
    out.push(calc(
        "calc-lowercase-name",
        "pdfium-src:ApplyNamedOperation",
        "AFSimple_Calculate(\"sum\", new Array(\"a\", \"c\"));",
        &f,
        "13",
    ));
    out.push(calc(
        "calc-rounding-6dp",
        "pdfium-src:AFSimple_Calculate",
        "AFSimple_Calculate(\"PRD\", new Array(\"x\", \"y\"));",
        &[("x", "0.1"), ("y", "0.2")],
        "0.02",
    ));
    out.push(calc(
        "calc-float-noise",
        "pdfium-src:AFSimple_Calculate",
        "AFSimple_Calculate(\"SUM\", new Array(\"x\", \"y\"));",
        &[("x", "0.1"), ("y", "0.2")],
        "0.3",
    ));
    out.push(calc(
        "calc-negative",
        s,
        "AFSimple_Calculate(\"SUM\", new Array(\"x\", \"y\"));",
        &[("x", "-5"), ("y", "2")],
        "-3",
    ));
    out.push(calc(
        "calc-decimal-comma",
        "adobe:AFMakeNumber",
        "AFSimple_Calculate(\"SUM\", new Array(\"x\", \"y\"));",
        &[("x", "1,5"), ("y", "2")],
        "3.5",
    ));
    out.push(calc(
        "calc-text-field-is-zero",
        "adobe:AFMakeNumber",
        "AFSimple_Calculate(\"SUM\", new Array(\"x\", \"y\"));",
        &[("x", "abc"), ("y", "2")],
        "2",
    ));
    out.push(calc(
        "calc-parent-name",
        s,
        "AFSimple_Calculate(\"SUM\", new Array(\"Sched\"));",
        &[("Sched.a", "1"), ("Sched.b", "2"), ("Other", "9")],
        "3",
    ));
    out.push(calc(
        "calc-min-first-from-second-name",
        "pdfium-src:AFSimple_Calculate",
        "AFSimple_Calculate(\"MIN\", new Array(\"zzz\", \"a\", \"c\"));",
        &f,
        "3",
    ));
    out.push(calc(
        "calc-large",
        s,
        "AFSimple_Calculate(\"SUM\", new Array(\"x\", \"y\"));",
        &[("x", "1000000"), ("y", "2500000.5")],
        "3500000.5",
    ));
}

fn merge_and_misc(out: &mut Vec<Case>) {
    let m = |id: &str, source: &str, value, change, sel, commit, expect| Case {
        id: id.into(),
        source: source.into(),
        note: "",
        kind: Kind::Merge {
            value,
            change,
            sel,
            commit,
            expect,
        },
    };
    let a = "adobe:AFMergeChange";
    out.push(m("merge-append", a, "hello", "!", (5, 5), false, "hello!"));
    out.push(m("merge-insert", a, "hllo", "e", (1, 1), false, "hello"));
    out.push(m(
        "merge-replace-selection",
        a,
        "hello",
        "XY",
        (1, 3),
        false,
        "hXYlo",
    ));
    out.push(m(
        "merge-delete-selection",
        a,
        "hello",
        "",
        (1, 4),
        false,
        "ho",
    ));
    out.push(m("merge-prepend", a, "ello", "h", (0, 0), false, "hello"));
    out.push(m(
        "merge-commit-returns-value",
        "pdfium:public_methods.in:629",
        "one",
        "A",
        (0, 0),
        true,
        "one",
    ));
    out.push(m(
        "merge-pdfium-noncommit",
        "pdfium:public_methods.in:631",
        "one",
        "A",
        (0, 0),
        false,
        "Aone",
    ));
    out.push(m(
        "merge-selection-past-end",
        "pdfium-src:CalcMergedString",
        "abc",
        "Z",
        (1, 99),
        false,
        "aZ",
    ));
    out.push(m(
        "merge-start-past-end",
        "pdfium-src:CalcMergedString",
        "abc",
        "Z",
        (99, 99),
        false,
        "abcZ",
    ));
    out.push(m(
        "merge-unicode",
        a,
        "caf\u{e9}",
        "s",
        (4, 4),
        false,
        "caf\u{e9}s",
    ));
    let n = |id: &str, source: &str, input, expect| Case {
        id: id.into(),
        source: source.into(),
        note: "",
        kind: Kind::MakeNumber { input, expect },
    };
    out.push(n(
        "makenumber-text",
        "pdfium:public_methods.in:197",
        "2blooey",
        0.0,
    ));
    out.push(n(
        "makenumber-int",
        "pdfium:public_methods.in:198",
        "1",
        1.0,
    ));
    out.push(n(
        "makenumber-dec",
        "pdfium:public_methods.in:199",
        "1.2",
        1.2,
    ));
    out.push(n(
        "makenumber-comma",
        "pdfium:public_methods.in:200",
        "1,2",
        1.2,
    ));
    out.push(n("makenumber-empty", "adobe:AFMakeNumber", "", 0.0));
    out.push(n("makenumber-neg", "adobe:AFMakeNumber", "-4.5", -4.5));
}

pub fn all_cases() -> Vec<Case> {
    let mut v = Vec::new();
    pdfium_format_cases(&mut v);
    number_cases(&mut v);
    percent_extra(&mut v);
    date_cases(&mut v);
    parse_print_cases(&mut v);
    special_cases(&mut v);
    keystroke_cases(&mut v);
    calc_cases(&mut v);
    merge_and_misc(&mut v);
    v
}

fn run(c: &Case) -> Result<(), String> {
    let fields: HashMap<String, String> = HashMap::new();
    match &c.kind {
        Kind::Format {
            script,
            init,
            expect,
            color,
        } => {
            let mut ev = Event::format(init);
            run_script(script, &mut ev, &fields, &env()).map_err(|e| format!("{e}"))?;
            if &ev.value != expect {
                return Err(format!("value {:?}, expected {:?}", ev.value, expect));
            }
            if let Some(col) = color
                && ev.text_color != Some(*col)
            {
                return Err(format!("colour {:?}, expected {:?}", ev.text_color, col));
            }
        }
        Kind::Key {
            script,
            value,
            change,
            sel,
            commit,
            rc,
            change_out,
            alert,
        } => {
            let mut ev = if *commit {
                Event::commit(value)
            } else {
                Event::keystroke(value, change, sel.0, sel.1)
            };
            run_script(script, &mut ev, &fields, &env()).map_err(|e| format!("{e}"))?;
            if ev.rc != *rc {
                return Err(format!("rc {}, expected {}", ev.rc, rc));
            }
            if let Some(co) = change_out
                && ev.change != *co
            {
                return Err(format!("change {:?}, expected {:?}", ev.change, co));
            }
            if *commit && *alert != !ev.alerts.is_empty() {
                return Err(format!("alerts {:?}, expected alert={}", ev.alerts, alert));
            }
            if *commit && ev.value != *value {
                return Err("commit validation must not modify the value".into());
            }
        }
        Kind::Calc {
            script,
            fields,
            expect,
        } => {
            let form = MapForm(
                fields
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            );
            let mut ev = Event::format("");
            run_script(script, &mut ev, &form, &env()).map_err(|e| format!("{e}"))?;
            if &ev.value != expect {
                return Err(format!("value {:?}, expected {:?}", ev.value, expect));
            }
        }
        Kind::Merge {
            value,
            change,
            sel,
            commit,
            expect,
        } => {
            let mut ev = Event::keystroke(value, change, sel.0, sel.1);
            ev.will_commit = *commit;
            let got = merge_change(&ev);
            if &got != expect {
                return Err(format!("merged {got:?}, expected {expect:?}"));
            }
        }
        Kind::MakeNumber { input, expect } => {
            let got = papyrine_forms::numfmt::make_number(input);
            if (got - expect).abs() > 1e-12 {
                return Err(format!("number {got}, expected {expect}"));
            }
        }
        Kind::ParsePrint {
            value,
            parse_fmt,
            print_fmt,
            expect,
        } => {
            let parsed = papyrine_forms::datefmt::parse_date_ex(value, parse_fmt, &env());
            let got = parsed.map(|d| papyrine_forms::datefmt::print_date(&d, print_fmt));
            if got.as_deref() != *expect {
                return Err(format!("got {got:?}, expected {expect:?}"));
            }
        }
    }
    Ok(())
}

#[test]
fn af_reference_cases() {
    let cases = all_cases();
    let mut failures = Vec::new();
    for c in &cases {
        if let Err(e) = run(c) {
            failures.push(format!("{} [{}]: {}", c.id, c.source, e));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

#[test]
fn at_least_200_reference_cases_with_sources() {
    let cases = all_cases();
    assert!(cases.len() >= 200, "only {} cases", cases.len());
    assert!(cases.iter().all(|c| !c.source.is_empty()));
    let mut ids: Vec<&str> = cases.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    let before = ids.len();
    ids.dedup();
    assert_eq!(before, ids.len(), "duplicate case ids");
    // Optional dump used for the Spike 0.4 write-up.
    if let Ok(path) = std::env::var("AF_REF_DUMP") {
        let mut out = String::from("id\tsource\tstatus\tnote\n");
        for c in &cases {
            let status = if c.note.starts_with("DISAGREE") {
                "references-disagree"
            } else {
                "agree"
            };
            out.push_str(&format!("{}\t{}\t{}\t{}\n", c.id, c.source, status, c.note));
        }
        std::fs::write(path, out).expect("write dump");
    }
}

#[test]
fn alerts_carry_acrobat_wording() {
    let mut ev = Event::commit("5");
    run_script(
        "AFRange_Validate(true, 2, true, 4);",
        &mut ev,
        &HashMap::new(),
        &env(),
    )
    .unwrap();
    assert_eq!(
        ev.alerts,
        vec![Alert::RangeBetween {
            lo: "2".into(),
            hi: "4".into()
        }]
    );
    assert_eq!(
        ev.alerts[0].message(),
        "Invalid value: must be greater than or equal to 2 and less than or equal to 4."
    );
}
