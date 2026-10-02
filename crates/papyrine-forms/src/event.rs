//! The subset of the Acrobat `event` object that the AF functions touch, plus
//! the side effects they can have (rejecting input, alerts, text colour).

/// RGB text colour set through `event.target.textColor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextColor {
    Black,
    Red,
}

/// A message Acrobat would show with `app.alert`. Hosts may localise these.
#[derive(Debug, Clone, PartialEq)]
pub enum Alert {
    /// Number or special-format commit with unparsable content.
    InvalidValue,
    InvalidDate {
        format: String,
    },
    RangeBetween {
        lo: String,
        hi: String,
    },
    RangeGreater {
        lo: String,
    },
    RangeLess {
        hi: String,
    },
    MaskTooLong,
    MaskInvalid,
}

impl Alert {
    /// Default English text, matching Acrobat's wording where it is known.
    pub fn message(&self) -> String {
        match self {
            Alert::InvalidValue => {
                "Invalid value: the entry does not match the field format.".into()
            }
            Alert::InvalidDate { format } => format!(
                "Invalid date/time: please ensure that the date/time exists. Field should match format {format}"
            ),
            Alert::RangeBetween { lo, hi } => format!(
                "Invalid value: must be greater than or equal to {lo} and less than or equal to {hi}."
            ),
            Alert::RangeGreater { lo } => {
                format!("Invalid value: must be greater than or equal to {lo}.")
            }
            Alert::RangeLess { hi } => {
                format!("Invalid value: must be less than or equal to {hi}.")
            }
            Alert::MaskTooLong => "The value entered is too long for the field's format.".into(),
            Alert::MaskInvalid => {
                "The value entered does not match the format of the field.".into()
            }
        }
    }
}

/// Mutable event state shared by a run of AF calls.
///
/// Selection offsets and string lengths are counted in Unicode scalar values
/// (Acrobat counts UTF-16 code units; the two agree for the BMP).
#[derive(Debug, Clone)]
pub struct Event {
    pub value: String,
    pub change: String,
    pub sel_start: usize,
    pub sel_end: usize,
    pub will_commit: bool,
    /// Cleared by a keystroke/validate action to reject the input.
    pub rc: bool,
    /// Fully qualified name of the field the event targets.
    pub target_name: String,
    pub text_color: Option<TextColor>,
    pub alerts: Vec<Alert>,
}

impl Event {
    fn blank(value: &str) -> Self {
        Event {
            value: value.to_string(),
            change: String::new(),
            sel_start: 0,
            sel_end: 0,
            will_commit: false,
            rc: true,
            target_name: String::new(),
            text_color: None,
            alerts: Vec::new(),
        }
    }

    /// Event for a format or calculate action.
    pub fn format(value: &str) -> Self {
        Self::blank(value)
    }

    /// Event for a validate action or a keystroke commit.
    pub fn commit(value: &str) -> Self {
        Event {
            will_commit: true,
            ..Self::blank(value)
        }
    }

    /// Event for one keystroke: `value` is the field text before the change.
    pub fn keystroke(value: &str, change: &str, sel_start: usize, sel_end: usize) -> Self {
        Event {
            change: change.to_string(),
            sel_start,
            sel_end,
            ..Self::blank(value)
        }
    }
}

/// `AFMergeChange`: the field text as it would be after applying the change.
pub fn merge_change(ev: &Event) -> String {
    if ev.will_commit {
        return ev.value.clone();
    }
    let v: Vec<char> = ev.value.chars().collect();
    let start = ev.sel_start.min(v.len());
    let end = ev.sel_end.min(v.len()).max(start);
    let mut s: String = v[..start].iter().collect();
    s.push_str(&ev.change);
    s.extend(&v[end..]);
    s
}

/// Errors that make an AF call throw in Acrobat (the event is left alone).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AfError {
    /// Wrong argument count or kind.
    Param,
    /// Argument outside the accepted range.
    Value,
}

impl std::fmt::Display for AfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AfError::Param => write!(f, "invalid parameters"),
            AfError::Value => write!(f, "invalid value"),
        }
    }
}

impl std::error::Error for AfError {}

/// Field lookup used by `AFSimple_Calculate`.
pub trait FieldLookup {
    /// Values of every field whose full name matches `name` (a parent name
    /// matches all its descendants). Empty if none. `None` entries are fields
    /// with no numeric content (buttons, unchecked boxes, lists with several
    /// selections).
    fn values(&self, name: &str) -> Vec<Option<String>>;
}

/// A trivial in-memory lookup.
impl FieldLookup for std::collections::HashMap<String, String> {
    fn values(&self, name: &str) -> Vec<Option<String>> {
        self.get(name)
            .map(|v| vec![Some(v.clone())])
            .unwrap_or_default()
    }
}
