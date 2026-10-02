//! `AFSpecial_*`: zip, zip+4, phone, SSN and arbitrary masks (`9 A O X`).

use crate::event::{Alert, Event};

#[derive(Clone, Copy, PartialEq)]
enum Case {
    Keep,
    Upper,
    Lower,
}

fn tr(c: char, m: Case) -> char {
    match m {
        Case::Lower if c.is_ascii_uppercase() => c.to_ascii_lowercase(),
        Case::Upper if c.is_ascii_lowercase() => c.to_ascii_uppercase(),
        _ => c,
    }
}

/// `util.printx(format, source)`: apply an input mask to `source`.
///
/// `9` digit, `A` letter, `X` letter or digit, `*` any, `?` any one char,
/// `\` escape, `>` `<` `=` upper/lower/preserve case; other characters are
/// literals. Source characters that do not satisfy a placeholder are skipped.
pub fn printx(format: &str, source: &str) -> String {
    let f: Vec<char> = format.chars().collect();
    let s: Vec<char> = source.chars().collect();
    let (mut fi, mut si) = (0usize, 0usize);
    let mut case = Case::Keep;
    let mut escaped = false;
    let mut out = String::new();
    // Each loop iteration advances fi or si, so termination is guaranteed.
    while fi < f.len() {
        let fc = f[fi];
        if escaped {
            escaped = false;
            out.push(fc);
            fi += 1;
            continue;
        }
        match fc {
            '\\' => {
                escaped = true;
                fi += 1;
            }
            '<' => {
                case = Case::Lower;
                fi += 1;
            }
            '>' => {
                case = Case::Upper;
                fi += 1;
            }
            '=' => {
                case = Case::Keep;
                fi += 1;
            }
            '?' => {
                if si < s.len() {
                    out.push(tr(s[si], case));
                    si += 1;
                }
                fi += 1;
            }
            'X' | 'A' | '9' => {
                if si < s.len() {
                    let c = s[si];
                    let ok = match fc {
                        'X' => c.is_ascii_alphanumeric(),
                        'A' => c.is_ascii_alphabetic(),
                        _ => c.is_ascii_digit(),
                    };
                    if ok {
                        out.push(if fc == '9' { c } else { tr(c, case) });
                        fi += 1;
                    }
                    si += 1;
                } else {
                    fi += 1;
                }
            }
            '*' => {
                if si < s.len() {
                    out.push(tr(s[si], case));
                    si += 1;
                } else {
                    fi += 1;
                }
            }
            other => {
                out.push(other);
                fi += 1;
            }
        }
    }
    out
}

/// `AFSpecial_Format(psf)`: 0 zip, 1 zip+4, 2 phone, 3 SSN. Other values
/// clear the field, as Acrobat does.
pub fn special_format(ev: &mut Event, psf: i64) {
    let mask = match psf {
        0 => "99999",
        1 => "99999-9999",
        2 => {
            if printx("9999999999", &ev.value).chars().count() >= 10 {
                "(999) 999-9999"
            } else {
                "999-9999"
            }
        }
        3 => "999-99-9999",
        _ => "",
    };
    ev.value = printx(mask, &ev.value);
}

fn mask_satisfied(c: char, m: char) -> bool {
    match m {
        '9' => c.is_ascii_digit(),
        'A' => c.is_ascii_alphabetic(),
        'O' => c.is_ascii_alphanumeric(),
        'X' => true,
        _ => c == m,
    }
}

fn reserved(m: char) -> bool {
    matches!(m, '9' | 'A' | 'O' | 'X')
}

/// `AFSpecial_KeystrokeEx(mask)`.
///
/// While typing, each inserted character must satisfy the mask position it
/// lands on; literal positions overwrite the typed character (so typing in a
/// phone mask supplies the dashes). On commit the whole value must match.
pub fn special_keystroke_ex(ev: &mut Event, mask: &str) {
    let m: Vec<char> = mask.chars().collect();
    if m.is_empty() {
        return;
    }
    let value: Vec<char> = ev.value.chars().collect();
    if ev.will_commit {
        if value.is_empty() {
            return;
        }
        if value.len() > m.len() {
            ev.alerts.push(Alert::MaskTooLong);
            ev.rc = false;
            return;
        }
        let matched = value
            .iter()
            .zip(&m)
            .take_while(|(c, k)| mask_satisfied(**c, **k))
            .count();
        // PDFium's rule: every value char must satisfy its mask char and the
        // value must be exactly as long as the mask.
        if matched != value.len() || value.len() != m.len() {
            ev.alerts.push(Alert::MaskInvalid);
            ev.rc = false;
        }
        return;
    }
    if ev.change.is_empty() {
        return;
    }
    let mut change: Vec<char> = ev.change.chars().collect();
    let combined = (value.len() + change.len() + ev.sel_start).saturating_sub(ev.sel_end);
    if combined > m.len() || ev.sel_start >= m.len() {
        ev.alerts.push(Alert::MaskTooLong);
        ev.rc = false;
        return;
    }
    for (at, c) in (ev.sel_start..).zip(change.iter_mut()) {
        let Some(&k) = m.get(at) else {
            ev.alerts.push(Alert::MaskTooLong);
            ev.rc = false;
            return;
        };
        if !reserved(k) {
            *c = k;
        }
        if !mask_satisfied(*c, k) {
            ev.rc = false;
            return;
        }
    }
    ev.change = change.into_iter().collect();
}

/// `AFSpecial_Keystroke(psf)`.
pub fn special_keystroke(ev: &mut Event, psf: i64) {
    let mask = match psf {
        0 => "99999",
        1 | 3 => "999999999",
        2 => {
            if ev.value.chars().count() + ev.change.chars().count() > 7 {
                "9999999999"
            } else {
                "9999999"
            }
        }
        _ => "",
    };
    special_keystroke_ex(ev, mask);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printx_cases() {
        assert_eq!(printx("99999-9999", ""), "-");
        assert_eq!(printx("(999) 999-9999", "5551234567"), "(555) 123-4567");
        assert_eq!(printx(">AAA", "abc"), "ABC");
        assert_eq!(printx("999-99-9999", "12a3-45 6789"), "123-45-6789");
    }
}
