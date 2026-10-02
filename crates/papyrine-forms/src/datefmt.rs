//! Date and time formatting, parsing and keystroke validation
//! (`AFDate_*`, `AFTime_*`). All arithmetic is proleptic Gregorian wall-clock
//! time; there is no time-zone handling because AF formats never shift zones.

use crate::event::{Alert, Event};

pub const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
pub const FULL_MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
pub const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub const FULL_DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// `AFDate_Format(index)` patterns.
pub const DATE_FORMATS: [&str; 14] = [
    "m/d",
    "m/d/yy",
    "mm/dd/yy",
    "mm/yy",
    "d-mmm",
    "d-mmm-yy",
    "dd-mmm-yy",
    "yy-mm-dd",
    "mmm-yy",
    "mmmm-yy",
    "mmm d, yyyy",
    "mmmm d, yyyy",
    "m/d/yy h:MM tt",
    "m/d/yy HH:MM",
];

/// `AFTime_Format(index)` patterns.
pub const TIME_FORMATS: [&str; 4] = ["HH:MM", "h:MM tt", "HH:MM:ss", "h:MM:ss tt"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl DateTime {
    pub const fn ymd(year: i32, month: u32, day: u32) -> Self {
        DateTime {
            year,
            month,
            day,
            hour: 0,
            minute: 0,
            second: 0,
        }
    }

    /// 0 = Sunday.
    pub fn weekday(&self) -> usize {
        let d = days_from_civil(self.year, self.month, self.day);
        (d + 4).rem_euclid(7) as usize
    }
}

/// What "now" is. Fields a value does not supply (for example the year in
/// `m/d`) are taken from `today`; the time of day always defaults to 00:00:00.
#[derive(Debug, Clone, Copy)]
pub struct DateEnv {
    pub today: DateTime,
}

impl Default for DateEnv {
    fn default() -> Self {
        DateEnv {
            today: DateTime::ymd(2000, 1, 1),
        }
    }
}

pub fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn valid(dt: &DateTime) -> bool {
    (1..=12).contains(&dt.month)
        && dt.day >= 1
        && dt.day <= days_in_month(dt.year, dt.month)
        && dt.hour <= 24
        && dt.minute <= 60
        && dt.second <= 60
}

// ---------------------------------------------------------------- printing

/// `util.printd`-style formatting as used by `AFDate_FormatEx`.
///
/// Tokens: `d dd ddd dddd`, `m mm mmm mmmm`, `yy yyyy`, `h hh` (12-hour),
/// `H HH` (24-hour), `M MM` (minutes), `s ss`, `t tt` (a/p, am/pm). Other
/// characters are copied.
pub fn print_date(dt: &DateTime, format: &str) -> String {
    let f: Vec<char> = format.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    let h12 = if dt.hour.is_multiple_of(12) {
        12
    } else {
        dt.hour % 12
    };
    let pm = dt.hour >= 12;
    while i < f.len() {
        let c = f[i];
        if !matches!(c, 'y' | 'm' | 'd' | 'H' | 'h' | 'M' | 's' | 't') {
            out.push(c);
            i += 1;
            continue;
        }
        let mut n = 1;
        while i + n < f.len() && f[i + n] == c {
            n += 1;
        }
        let rep = |k: usize| std::iter::repeat_n(c, k).collect::<String>();
        match (c, n) {
            ('y', 1) => out.push('y'),
            ('y', 2) => out.push_str(&format!("{:02}", dt.year.rem_euclid(100))),
            ('y', 4) => out.push_str(&format!("{:04}", dt.year)),
            ('m', 1) => out.push_str(&dt.month.to_string()),
            ('m', 2) => out.push_str(&format!("{:02}", dt.month)),
            ('m', 3) => out.push_str(MONTHS[(dt.month as usize).clamp(1, 12) - 1]),
            ('m', 4) => out.push_str(FULL_MONTHS[(dt.month as usize).clamp(1, 12) - 1]),
            ('d', 1) => out.push_str(&dt.day.to_string()),
            ('d', 2) => out.push_str(&format!("{:02}", dt.day)),
            ('d', 3) => out.push_str(DAYS[dt.weekday()]),
            ('d', 4) => out.push_str(FULL_DAYS[dt.weekday()]),
            ('H', 1) => out.push_str(&dt.hour.to_string()),
            ('H', 2) => out.push_str(&format!("{:02}", dt.hour)),
            ('h', 1) => out.push_str(&h12.to_string()),
            ('h', 2) => out.push_str(&format!("{h12:02}")),
            ('M', 1) => out.push_str(&dt.minute.to_string()),
            ('M', 2) => out.push_str(&format!("{:02}", dt.minute)),
            ('s', 1) => out.push_str(&dt.second.to_string()),
            ('s', 2) => out.push_str(&format!("{:02}", dt.second)),
            ('t', 1) => out.push(if pm { 'p' } else { 'a' }),
            ('t', 2) => out.push_str(if pm { "pm" } else { "am" }),
            _ => out.push_str(&rep(n)),
        }
        i += n;
    }
    out
}

// ----------------------------------------------------------------- parsing

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseStatus {
    Ok(DateTime),
    BadFormat,
    BadDate,
}

fn parse_int(v: &[char], start: usize, max: usize) -> (i64, usize) {
    let mut n = 0i64;
    let mut skip = 0;
    for &c in v.iter().skip(start) {
        if skip > 10 || !c.is_ascii_digit() {
            break;
        }
        n = n * 10 + i64::from(c.to_digit(10).unwrap_or(0));
        skip += 1;
        if skip >= max {
            break;
        }
    }
    (n, skip)
}

fn sub_word_len(v: &[char], start: usize) -> usize {
    v.iter()
        .skip(start)
        .take_while(|c| c.is_alphanumeric())
        .count()
}

/// Parse `value` strictly against `format`, filling unspecified date fields
/// from `env.today`. Separators in the format skip exactly one input
/// character, so `mm/dd/yyyy` reads `12302015` as 12/02/2015.
pub fn parse_with_format(value: &str, format: &str, env: &DateEnv) -> ParseStatus {
    let v: Vec<char> = value.chars().collect();
    let f: Vec<char> = format.chars().collect();
    let t = env.today;
    if f.is_empty() || v.is_empty() {
        return ParseStatus::Ok(t);
    }
    let (mut year, mut month, mut day) = (i64::from(t.year), i64::from(t.month), i64::from(t.day));
    let (mut hour, mut minute, mut second) = (0i64, 0i64, 0i64);
    #[derive(PartialEq)]
    enum Ap {
        None,
        Am,
        Pm,
    }
    let mut ap = Ap::None;
    let (mut fi, mut vi) = (0usize, 0usize);
    let mut bad_format = false;
    while fi < f.len() {
        let fc = f[fi];
        match fc {
            ':' | '.' | '-' | '\\' | '/' => {
                fi += 1;
                vi += 1;
            }
            'y' | 'm' | 'd' | 'H' | 'h' | 'M' | 's' | 't' => {
                let old = vi;
                let mut n = 1;
                while fi + n < f.len() && f[fi + n] == fc {
                    n += 1;
                }
                if n <= 2 {
                    match fc {
                        'y' => {
                            if n == 1 {
                                vi += 1;
                            } else {
                                let (x, s) = parse_int(&v, vi, 2);
                                year = x;
                                vi += s;
                            }
                        }
                        'm' | 'd' | 'H' | 'h' | 'M' | 's' => {
                            let (x, s) = parse_int(&v, vi, 2);
                            match fc {
                                'm' => month = x,
                                'd' => day = x,
                                'H' | 'h' => hour = x,
                                'M' => minute = x,
                                _ => second = x,
                            }
                            vi += s;
                        }
                        _ => {
                            if vi + n <= v.len() {
                                let m: String =
                                    v[vi..vi + n].iter().collect::<String>().to_lowercase();
                                let (p, a) = if n == 1 { ("p", "a") } else { ("pm", "am") };
                                if m == p {
                                    ap = Ap::Pm;
                                    vi += n;
                                } else if m == a {
                                    ap = Ap::Am;
                                    vi += n;
                                }
                            }
                        }
                    }
                    fi += n;
                } else if n == 3 {
                    match fc {
                        'm' => {
                            let len = sub_word_len(&v, vi);
                            let mut found = false;
                            if len == 3 {
                                let w: String =
                                    v[vi..vi + 3].iter().collect::<String>().to_lowercase();
                                if let Some(k) = MONTHS.iter().position(|m| m.to_lowercase() == w) {
                                    month = k as i64 + 1;
                                    fi += 3;
                                    vi += len;
                                    found = true;
                                }
                            }
                            if !found {
                                let (x, s) = parse_int(&v, vi, 3);
                                month = x;
                                fi += 3;
                                vi += s;
                            }
                        }
                        'y' => {}
                        _ => {
                            fi += 3;
                            vi += 3;
                        }
                    }
                } else if n == 4 {
                    match fc {
                        'y' => {
                            let (x, s) = parse_int(&v, vi, 4);
                            year = x;
                            fi += 4;
                            vi += s;
                        }
                        'm' => {
                            let len = sub_word_len(&v, vi);
                            let mut found = false;
                            if len <= 9 && len > 0 {
                                let w: String =
                                    v[vi..vi + len].iter().collect::<String>().to_lowercase();
                                if let Some(k) = FULL_MONTHS
                                    .iter()
                                    .position(|m| m.to_lowercase().contains(&w))
                                {
                                    month = k as i64 + 1;
                                    fi += 4;
                                    vi += len;
                                    found = true;
                                }
                            }
                            if !found {
                                let (x, s) = parse_int(&v, vi, 4);
                                month = x;
                                fi += 4;
                                vi += s;
                            }
                        }
                        _ => {
                            fi += 4;
                            vi += 4;
                        }
                    }
                } else {
                    if vi >= v.len() || f[fi] != v[vi] {
                        bad_format = true;
                        break;
                    }
                    fi += 1;
                    vi += 1;
                }
                if old == vi {
                    bad_format = true;
                    break;
                }
            }
            _ => {
                if v.len() <= vi {
                    break;
                }
                if f[fi] != v[vi] {
                    bad_format = true;
                    break;
                }
                fi += 1;
                vi += 1;
            }
        }
    }
    if bad_format {
        return ParseStatus::BadFormat;
    }
    if ap != Ap::None {
        hour = (hour % 12) + if ap == Ap::Pm { 12 } else { 0 };
    }
    if (0..100).contains(&year) {
        year += if year < 50 { 2000 } else { 1900 };
    }
    build(year, month, day, hour, minute, second).map_or(ParseStatus::BadDate, ParseStatus::Ok)
}

fn build(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> Option<DateTime> {
    let dt = DateTime {
        year: i32::try_from(y).ok()?,
        month: u32::try_from(mo).ok()?,
        day: u32::try_from(d).ok()?,
        hour: u32::try_from(h).ok()?,
        minute: u32::try_from(mi).ok()?,
        second: u32::try_from(s).ok()?,
    };
    valid(&dt).then_some(dt)
}

/// Free-form fallback: pull up to three numbers out of the text and guess
/// their roles (`ParseDate` in PDFium). The bool is "wrong format".
fn parse_free(value: &str, env: &DateEnv) -> (Option<DateTime>, bool) {
    let v: Vec<char> = value.chars().collect();
    let mut num = [0i64; 3];
    let mut count = 0;
    let mut i = 0;
    while i < v.len() && count <= 2 {
        if v[i].is_ascii_digit() {
            let (n, s) = parse_int(&v, i, 4);
            num[count] = n;
            count += 1;
            i += s;
        } else {
            i += 1;
        }
    }
    let t = env.today;
    let (mut y, mut m, mut d) = (i64::from(t.year), i64::from(t.month), i64::from(t.day));
    let vm = |x: i64| (1..=12).contains(&x);
    let vd = |x: i64| (1..=31).contains(&x);
    // PDFium falls back to "today" when no reading fits; we reject instead.
    let fits = match count {
        2 if vm(num[0]) && vd(num[1]) => {
            m = num[0];
            d = num[1];
            true
        }
        2 if vd(num[0]) && vm(num[1]) => {
            d = num[0];
            m = num[1];
            true
        }
        3 if num[0] > 12 && vm(num[1]) && vd(num[2]) => {
            y = num[0];
            m = num[1];
            d = num[2];
            true
        }
        3 if vm(num[0]) && vd(num[1]) && num[2] > 31 => {
            m = num[0];
            d = num[1];
            y = num[2];
            true
        }
        3 if vd(num[0]) && vm(num[1]) && num[2] > 31 => {
            d = num[0];
            m = num[1];
            y = num[2];
            true
        }
        2 | 3 => false,
        _ => return (None, true),
    };
    if !fits {
        return (None, false);
    }
    if (0..100).contains(&y) {
        y += if y < 50 { 2000 } else { 1900 };
    }
    (build(y, m, d, 0, 0, 0), false)
}

/// A pragmatic subset of JavaScript's `Date.parse` for human-entered dates:
/// ISO `YYYY-MM-DD[THH:MM[:SS]]`, `M/D/YYYY [h:mm[:ss] [am|pm]]` and month-name
/// forms such as `Aug 11, 2009` or `11 August 2009`.
pub fn js_date_parse(s: &str) -> Option<DateTime> {
    let s = s.trim();
    let b: Vec<char> = s.chars().collect();
    // ISO 8601 date
    if b.len() >= 10 && b[4] == '-' && b[7] == '-' {
        let digits = |r: std::ops::Range<usize>| -> Option<i64> {
            let t: String = b[r].iter().collect();
            if t.chars().all(|c| c.is_ascii_digit()) {
                t.parse().ok()
            } else {
                None
            }
        };
        if let (Some(y), Some(m), Some(d)) = (digits(0..4), digits(5..7), digits(8..10)) {
            let (mut h, mut mi, mut sec) = (0, 0, 0);
            if b.len() >= 16 && (b[10] == 'T' || b[10] == ' ') && b[13] == ':' {
                h = digits(11..13)?;
                mi = digits(14..16)?;
                if b.len() >= 19 && b[16] == ':' {
                    sec = digits(17..19)?;
                }
            }
            return build(y, m, d, h, mi, sec);
        }
    }
    let mut month: Option<i64> = None;
    let mut nums: Vec<i64> = Vec::new();
    let (mut h, mut mi, mut sec) = (0i64, 0i64, 0i64);
    let mut ap: Option<bool> = None;
    let mut have_time = false;
    for tok in s.split(|c: char| c.is_whitespace() || c == ',' || c == '/' || c == '-' || c == '.')
    {
        if tok.is_empty() {
            continue;
        }
        if tok.contains(':') {
            let parts: Vec<&str> = tok.split(':').collect();
            if parts.len() < 2 || parts.len() > 3 {
                return None;
            }
            h = parts[0].parse().ok()?;
            mi = parts[1].parse().ok()?;
            if let Some(p) = parts.get(2) {
                sec = p.parse().ok()?;
            }
            have_time = true;
        } else if tok.chars().all(|c| c.is_ascii_digit()) {
            nums.push(tok.parse().ok()?);
        } else {
            let l = tok.to_lowercase();
            if l == "am" || l == "pm" {
                ap = Some(l == "pm");
            } else if l.len() >= 3 && FULL_MONTHS.iter().any(|m| m.to_lowercase().starts_with(&l)) {
                let k = FULL_MONTHS
                    .iter()
                    .position(|m| m.to_lowercase().starts_with(&l))?;
                month = Some(k as i64 + 1);
            } else if DAYS
                .iter()
                .any(|d| d.to_lowercase() == l.get(..3).unwrap_or(""))
                || matches!(l.as_str(), "gmt" | "utc" | "z")
            {
                // weekday names and zone names carry no information we use
            } else {
                return None;
            }
        }
    }
    if let Some(p) = ap {
        h = (h % 12) + if p { 12 } else { 0 };
    }
    let _ = have_time;
    let (y, m, d) = match (month, nums.as_slice()) {
        (Some(m), [d, y]) if *d <= 31 => (*y, m, *d),
        (Some(m), [y, d]) if *d <= 31 && *y > 31 => (*y, m, *d),
        (None, [m, d, y]) => (*y, *m, *d),
        _ => return None,
    };
    let y = if (0..100).contains(&y) {
        y + if y < 50 { 2000 } else { 1900 }
    } else {
        y
    };
    build(y, m, d, h, mi, sec)
}

/// `AFParseDateEx`: format-driven parse with the same fallbacks Acrobat-like
/// engines use. Returns `None` when the text is not a date.
pub fn parse_date_ex(value: &str, format: &str, env: &DateEnv) -> Option<DateTime> {
    match parse_with_format(value, format, env) {
        ParseStatus::Ok(d) => return Some(d),
        ParseStatus::BadDate | ParseStatus::BadFormat => {
            if let Some(d) = js_date_parse(value) {
                return Some(d);
            }
        }
    }
    match parse_free(value, env) {
        (d, false) => d,
        _ => None,
    }
}

/// `AFDate_FormatEx` (and, with the lookup done by the caller, `AFDate_Format`,
/// `AFTime_Format`, `AFTime_FormatEx`).
pub fn date_format_ex(ev: &mut Event, format: &str, env: &DateEnv) {
    if ev.value.is_empty() {
        return;
    }
    match parse_date_ex(&ev.value, format, env) {
        Some(d) => ev.value = print_date(&d, format),
        None => ev.alerts.push(Alert::InvalidDate {
            format: format.to_string(),
        }),
    }
}

/// `AFDate_KeystrokeEx` (and the time variants): on commit, the text must
/// parse as a date in the given format.
pub fn date_keystroke_ex(ev: &mut Event, format: &str, env: &DateEnv) {
    if !ev.will_commit || ev.value.is_empty() {
        return;
    }
    if parse_date_ex(&ev.value, format, env).is_none() {
        ev.alerts.push(Alert::InvalidDate {
            format: format.to_string(),
        });
        ev.rc = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_basic() {
        let d = DateTime {
            year: 2023,
            month: 3,
            day: 7,
            hour: 0,
            minute: 5,
            second: 9,
        };
        assert_eq!(print_date(&d, "mm/dd/yyyy"), "03/07/2023");
        assert_eq!(print_date(&d, "m/d/yy h:MM tt"), "3/7/23 12:05 am");
        assert_eq!(
            print_date(&d, "dddd, mmmm d, yyyy"),
            "Tuesday, March 7, 2023"
        );
    }

    #[test]
    fn parse_basic() {
        let env = DateEnv {
            today: DateTime::ymd(2014, 5, 9),
        };
        assert_eq!(
            parse_date_ex("12/31/2020", "mm/dd/yyyy", &env),
            Some(DateTime::ymd(2020, 12, 31))
        );
        assert_eq!(parse_date_ex("2/30/2020", "m/d/yyyy", &env), None);
        assert_eq!(
            parse_date_ex("4/5", "m/d", &env),
            Some(DateTime::ymd(2014, 4, 5))
        );
        assert_eq!(
            parse_date_ex("Mar 7, 2023", "mm/dd/yyyy", &env),
            Some(DateTime::ymd(2023, 3, 7))
        );
    }
}
