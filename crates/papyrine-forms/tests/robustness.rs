//! No panics, no hangs on arbitrary input; nothing outside the grammar is
//! ever accepted. A fixed-seed xorshift generator keeps runs reproducible.

use std::collections::HashMap;

use papyrine_forms::datefmt::{DateTime, parse_date_ex, print_date};
use papyrine_forms::event::Event;
use papyrine_forms::lexer::tokenize;
use papyrine_forms::special::printx;
use papyrine_forms::{DateEnv, Verdict, analyze, recognize, run_script};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const FRAGMENTS: &[&str] = &[
    "AFNumber_Format",
    "AFNumber_Keystroke",
    "AFPercent_Format",
    "AFDate_FormatEx",
    "AFDate_KeystrokeEx",
    "AFTime_Format",
    "AFSpecial_Format",
    "AFSpecial_KeystrokeEx",
    "AFSimple_Calculate",
    "AFRange_Validate",
    "new",
    "Array",
    "true",
    "false",
    "null",
    "undefined",
    "var",
    "if",
    "function",
    "event",
    "this",
    "(",
    ")",
    "[",
    "]",
    "{",
    "}",
    ",",
    ";",
    ".",
    "=",
    "+",
    "-",
    "*",
    "/",
    "\"",
    "'",
    "\\",
    "`",
    "\n",
    "\r\n",
    " ",
    "0",
    "1",
    "2",
    "9",
    "1e9",
    "0x1f",
    ".5",
    "1e",
    "\"mm/dd/yyyy\"",
    "'$'",
    "\"\\u00e9\"",
    "\"\\x4\"",
    "/*",
    "*/",
    "//",
    "\u{2028}",
    "é",
    "\u{0}",
    "😀",
    "1e999",
    "99999999999999999999999999",
];

fn random_script(r: &mut Rng) -> String {
    let n = r.below(40);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(FRAGMENTS[r.below(FRAGMENTS.len())]);
        if r.below(3) == 0 {
            s.push(' ');
        }
    }
    s
}

fn random_text(r: &mut Rng, alphabet: &[char], max: usize) -> String {
    (0..r.below(max))
        .map(|_| alphabet[r.below(alphabet.len())])
        .collect()
}

#[test]
fn random_scripts_never_panic() {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    for _ in 0..40_000 {
        let s = random_script(&mut r);
        let _ = tokenize(&s);
        let _ = recognize(&s);
        let _ = analyze(&s);
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut r = Rng(42);
    for _ in 0..20_000 {
        let len = r.below(200);
        let bytes: Vec<u8> = (0..len).map(|_| r.next() as u8).collect();
        let s = String::from_utf8_lossy(&bytes).into_owned();
        let _ = analyze(&s);
    }
}

#[test]
fn mutated_valid_scripts_never_panic() {
    let seeds = [
        "AFNumber_Format(2, 0, 0, 0, \"$\", true);",
        "AFDate_FormatEx(\"mm/dd/yyyy\"); AFSpecial_KeystrokeEx(\"999-99-9999\");",
        "AFSimple_Calculate(\"SUM\", new Array(\"a\", \"b\"));",
    ];
    let mut r = Rng(7);
    for _ in 0..30_000 {
        let base = seeds[r.below(seeds.len())];
        let mut chars: Vec<char> = base.chars().collect();
        for _ in 0..1 + r.below(4) {
            let i = r.below(chars.len().max(1));
            match r.below(3) {
                0 if !chars.is_empty() => {
                    chars.remove(i);
                }
                1 => chars.insert(
                    i.min(chars.len()),
                    FRAGMENTS[r.below(FRAGMENTS.len())]
                        .chars()
                        .next()
                        .unwrap_or(' '),
                ),
                _ if !chars.is_empty() => {
                    chars[i] = FRAGMENTS[r.below(FRAGMENTS.len())]
                        .chars()
                        .next()
                        .unwrap_or(' ')
                }
                _ => {}
            }
        }
        let s: String = chars.into_iter().collect();
        let ev_value = random_text(&mut r, &['1', '2', '-', '.', ',', 'a', ' ', '/', ':'], 12);
        let mut ev = Event::format(&ev_value);
        let _ = run_script(
            &s,
            &mut ev,
            &HashMap::<String, String>::new(),
            &DateEnv::default(),
        );
    }
}

#[test]
fn runtime_functions_never_panic_on_extreme_input() {
    let mut r = Rng(1234);
    let alphabet: Vec<char> = "0123456789-+.,eE abcxyzAMPT:/\\'$%()9AOX<>=*?é😀"
        .chars()
        .collect();
    let scripts = [
        "AFNumber_Format(2, 0, 0, 0, \"$\", true);",
        "AFNumber_Format(-3, 4, 3, 0, \"\", false);",
        "AFNumber_Format(99999999999, 99, 99, 0, \"\", false);",
        "AFNumber_Keystroke(2, 0, 0, 0, \"\", true);",
        "AFPercent_Format(99999, 0);",
        "AFPercent_Format(512, 2, true);",
        "AFPercent_Format(-1, 0);",
        "AFPercent_Format(1, 99);",
        "AFDate_FormatEx(\"mmmm dddd yyyy hh:MM:ss tt\");",
        "AFDate_FormatEx(\"yyyyyyyyyyyyyyyy mmmmmmmmm\");",
        "AFDate_KeystrokeEx(\"m/d/yyyy\");",
        "AFDate_Format(-1);",
        "AFTime_Format(7);",
        "AFTime_Keystroke(1);",
        "AFSpecial_Format(2);",
        "AFSpecial_Format(-5);",
        "AFSpecial_KeystrokeEx(\"(999) 999-9999\");",
        "AFSpecial_KeystrokeEx(\">AAA<XXX=OO\\\\9\");",
        "AFRange_Validate(true, 1e308, true, -1e308);",
        "AFSimple_Calculate(\"AVG\", \"a,b,,c\");",
    ];
    for _ in 0..30_000 {
        let value = random_text(&mut r, &alphabet, 30);
        let change = random_text(&mut r, &alphabet, 5);
        let (a, b) = (r.below(40), r.below(40));
        for s in scripts {
            for mode in 0..3 {
                let mut ev = match mode {
                    0 => Event::format(&value),
                    1 => Event::commit(&value),
                    _ => Event::keystroke(&value, &change, a, b),
                };
                let _ = run_script(
                    s,
                    &mut ev,
                    &HashMap::<String, String>::new(),
                    &DateEnv::default(),
                );
            }
        }
        let _ = printx(&random_text(&mut r, &alphabet, 20), &value);
        let _ = parse_date_ex(
            &value,
            &random_text(&mut r, &alphabet, 14),
            &DateEnv::default(),
        );
        let dt = DateTime {
            year: (r.next() as i32).rem_euclid(20000) - 5000,
            month: r.below(20) as u32,
            day: r.below(40) as u32,
            hour: r.below(30) as u32,
            minute: r.below(70) as u32,
            second: r.below(70) as u32,
        };
        let _ = print_date(&dt, &random_text(&mut r, &alphabet, 20));
    }
}

#[test]
fn deep_nesting_and_huge_inputs_are_bounded() {
    let deep = format!(
        "AFSimple_Calculate(\"SUM\", {}\"a\"{});",
        "[".repeat(5000),
        "]".repeat(5000)
    );
    assert!(recognize(&deep).is_err());
    let many = "AFDate_FormatEx(\"mm/dd/yyyy\");".repeat(10_000);
    assert!(recognize(&many).is_err(), "statement cap");
    let huge = "a".repeat(2 << 20);
    assert!(recognize(&huge).is_err());
    let long_string = format!("AFDate_FormatEx(\"{}\");", "m".repeat(500_000));
    assert!(recognize(&long_string).is_ok());
    let mut ev = Event::format("3/7/2023");
    let _ = run_script(
        &long_string,
        &mut ev,
        &HashMap::<String, String>::new(),
        &DateEnv::default(),
    );
}

#[test]
fn only_allowlisted_scripts_are_accepted() {
    for s in [
        "AFNumber_Format(2, 0, 0, 0, \"$\", true); eval('x')",
        "AFNumber_Format(2, 0, 0, 0, \"$\", true) , 1",
        "(AFNumber_Format)(2, 0, 0, 0, \"$\", true);",
        "this.AFNumber_Format(2, 0, 0, 0, \"$\", true);",
        "AFNumber_Format(2, 0, 0, 0, \"$\", true)();",
        "AFNumber_Format(2, 0, 0, 0, \"$\" + x, true);",
        "AFNumber_Format(2, 0, 0, 0, `$`, true);",
        "AFNumber_Format(2, 0, 0, 0, \"$\", true) ? 1 : 2;",
        "AFNumber_Format(2, 0, 0, 0, \"$\", !0);",
    ] {
        assert!(!matches!(analyze(s), Verdict::Accepted(_)), "accepted: {s}");
    }
}
