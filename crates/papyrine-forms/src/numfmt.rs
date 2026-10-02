//! Number and percent formatting and keystroke validation, following
//! Acrobat's `AFNumber_*` and `AFPercent_*`.

use crate::event::{AfError, Alert, Event, TextColor};

/// Thousands separator and decimal mark for `sepStyle`.
/// 0 `1,234.56`, 1 `1234.56`, 2 `1.234,56`, 3 `1234,56`, 4+ `1'234.56`.
pub(crate) fn sep_style(style: i64) -> (Option<char>, char) {
    match style {
        0 => (Some(','), '.'),
        1 => (None, '.'),
        2 => (Some('.'), ','),
        3 => (None, ','),
        _ => (Some('\''), '.'),
    }
}

/// C `atof`: parse the longest numeric prefix, 0 if none.
pub fn atof(s: &str) -> f64 {
    let b = s.trim_start().as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let ds = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - ds;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let fs = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - fs;
    }
    if digits == 0 {
        return 0.0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    let t = s.trim_start();
    t[..i].parse::<f64>().unwrap_or(0.0)
}

/// JS `Number(string)`: NaN unless the whole string is a valid numeric literal.
pub fn js_to_number(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return u128::from_str_radix(h, 16)
            .map(|v| v as f64)
            .unwrap_or(f64::NAN);
    }
    if !t
        .bytes()
        .all(|c| c.is_ascii_digit() || matches!(c, b'.' | b'e' | b'E' | b'+' | b'-'))
    {
        return f64::NAN;
    }
    t.parse::<f64>().unwrap_or(f64::NAN)
}

/// ECMAScript `Number::toString`.
pub fn js_number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.is_infinite() {
        return if n > 0.0 {
            "Infinity".into()
        } else {
            "-Infinity".into()
        };
    }
    let sign = if n < 0.0 { "-" } else { "" };
    let e = format!("{:e}", n.abs());
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let digits: String = mant.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n10: i32 = exp.parse::<i32>().unwrap_or(0) + 1; // position of decimal point
    let body = if k <= n10 && n10 <= 21 {
        format!("{digits}{}", "0".repeat((n10 - k) as usize))
    } else if 0 < n10 && n10 <= 21 {
        format!("{}.{}", &digits[..n10 as usize], &digits[n10 as usize..])
    } else if -6 < n10 && n10 <= 0 {
        format!("0.{}{digits}", "0".repeat((-n10) as usize))
    } else {
        let ex = n10 - 1;
        let es = if ex < 0 {
            format!("-{}", -ex)
        } else {
            format!("+{ex}")
        };
        if k == 1 {
            format!("{digits}e{es}")
        } else {
            format!("{}.{}e{es}", &digits[..1], &digits[1..])
        }
    };
    format!("{sign}{body}")
}

/// `AFMakeNumber`: decimal comma tolerated, non-numbers become 0.
pub fn make_number(s: &str) -> f64 {
    let n = js_to_number(&s.replace(',', "."));
    if n.is_nan() { 0.0 } else { n }
}

fn insert_thousands(int_part: &str, sep: char) -> String {
    let len = int_part.chars().count();
    let mut out = String::with_capacity(len + len / 3);
    for (i, c) in int_part.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

/// Parameters of `AFNumber_Format`.
#[derive(Debug, Clone)]
pub struct NumberFormat {
    pub n_dec: i64,
    pub sep_style: i64,
    pub neg_style: i64,
    pub currency: String,
    pub prepend: bool,
}

/// `AFNumber_Format` over `event.value`.
pub fn number_format(ev: &mut Event, p: &NumberFormat) {
    let trimmed = ev.value.trim_matches(' ').to_string();
    if trimmed.is_empty() {
        return;
    }
    let dec = p.n_dec.unsigned_abs().min(15) as usize;
    let neg_style = if (0..4).contains(&p.neg_style) {
        p.neg_style
    } else {
        0
    };
    let (thou, mark) = sep_style(p.sep_style);

    let mut d = atof(&trimmed.replace(',', "."));
    if p.n_dec != 0 {
        d += 1e-15;
    }
    let negative = d < 0.0;
    let fixed = format!("{:.*}", dec, d.abs());
    let (int_part, frac) = match fixed.split_once('.') {
        Some((i, f)) => (i.to_string(), Some(f.to_string())),
        None => (fixed.clone(), None),
    };
    let mut s = match thou {
        Some(t) => insert_thousands(&int_part, t),
        None => int_part,
    };
    if let Some(f) = frac {
        s.push(mark);
        s.push_str(&f);
    }
    s = if p.prepend {
        format!("{}{s}", p.currency)
    } else {
        format!("{s}{}", p.currency)
    };
    if negative {
        match neg_style {
            0 | 1 => s.insert(0, '-'),
            2 | 3 => s = format!("({s})"),
            _ => {}
        }
    }
    if neg_style == 1 || neg_style == 3 {
        ev.text_color = Some(if negative {
            TextColor::Red
        } else {
            TextColor::Black
        });
    }
    ev.value = s;
}

/// `AFPercent_Format` over `event.value`.
pub fn percent_format(ev: &mut Event, n_dec: i64, sep: i64, prepend: bool) -> Result<(), AfError> {
    if n_dec < 0 || !(0..=49).contains(&sep) {
        return Err(AfError::Value);
    }
    if n_dec > 512 {
        ev.value = "%".into();
        return Ok(());
    }
    let t = ev.value.trim_matches(' ');
    let d = if t.is_empty() { 0.0 } else { atof(t) } * 100.0;
    let fixed = format!("{:.*}", n_dec as usize, d);
    let (thou, mark) = sep_style(sep);
    let (neg, digits) = match fixed.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, fixed.as_str()),
    };
    let (int_part, frac) = match digits.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (digits, None),
    };
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    match thou {
        Some(t) => s.push_str(&insert_thousands(int_part, t)),
        None => s.push_str(int_part),
    }
    if let Some(f) = frac {
        s.push(mark);
        s.push_str(f);
    }
    ev.value = if prepend {
        format!("%{s}")
    } else {
        format!("{s}%")
    };
    Ok(())
}

fn is_number(s: &str) -> bool {
    let t: Vec<char> = s.trim_matches(' ').chars().collect();
    let mut dot = false;
    let mut exp = false;
    let mut i = 0;
    while i < t.len() {
        let c = t[i];
        match c {
            '.' | ',' => {
                if dot {
                    return false;
                }
                dot = true;
            }
            '-' | '+' => {
                if i != 0 {
                    return false;
                }
            }
            'e' | 'E' => {
                if exp {
                    return false;
                }
                i += 1;
                if !matches!(t.get(i), Some('+' | '-')) {
                    return false;
                }
                exp = true;
            }
            c if c.is_ascii_digit() => {}
            _ => return false,
        }
        i += 1;
    }
    true
}

/// `AFNumber_Keystroke` / `AFPercent_Keystroke`: reject characters that cannot
/// be part of a number (clears `rc`); validate the whole text on commit.
pub fn number_keystroke(ev: &mut Event, sep_style_arg: i64) {
    if ev.will_commit {
        let t = ev.value.trim_matches(' ').replace(',', ".");
        if t.is_empty() {
            return;
        }
        if !is_number(&t) {
            ev.rc = false;
            ev.alerts.push(Alert::InvalidValue);
        }
        return;
    }
    let value: Vec<char> = ev.value.chars().collect();
    let start = ev.sel_start.min(value.len());
    let end = ev.sel_end.min(value.len()).max(start);
    let selected = &value[start..end];
    let mut has_sign = value.contains(&'-') && !selected.contains(&'-');
    if has_sign && !selected.is_empty() && ev.sel_start == 0 {
        ev.rc = false;
        return;
    }
    let style = if (0..4).contains(&sep_style_arg) {
        sep_style_arg
    } else {
        0
    };
    let sep = sep_style(style).1;
    let mut has_sep = value.contains(&sep);
    for (i, c) in ev.change.chars().enumerate() {
        if c == sep {
            if has_sep {
                ev.rc = false;
                return;
            }
            has_sep = true;
        } else if c == '-' {
            if has_sign || i != 0 || ev.sel_start != 0 {
                ev.rc = false;
                return;
            }
            has_sign = true;
        } else if !c.is_ascii_digit() {
            ev.rc = false;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_numbers() {
        assert_eq!(js_number_to_string(1.5), "1.5");
        assert_eq!(js_number_to_string(100.0), "100");
        assert_eq!(js_number_to_string(1e21), "1e+21");
        assert_eq!(js_number_to_string(1.5e-7), "1.5e-7");
        assert_eq!(js_number_to_string(0.000001), "0.000001");
        assert_eq!(js_number_to_string(-123.456), "-123.456");
        assert_eq!(js_number_to_string(579.0), "579");
        assert!(js_to_number("2blooey").is_nan());
        assert_eq!(js_to_number(" 12 "), 12.0);
        assert_eq!(atof("12abc"), 12.0);
        assert_eq!(atof("-.5x"), -0.5);
        assert_eq!(atof("abc"), 0.0);
        assert_eq!(atof("1e"), 1.0);
    }

    #[test]
    fn number_basic() {
        let mut ev = Event::format("1234.5");
        number_format(
            &mut ev,
            &NumberFormat {
                n_dec: 2,
                sep_style: 0,
                neg_style: 0,
                currency: "$".into(),
                prepend: true,
            },
        );
        assert_eq!(ev.value, "$1,234.50");
    }
}
