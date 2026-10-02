//! Startup trace. Subsystems wrap their initialization in a span named
//! `subsystem.<name>`; the host records a `first_paint` marker once the window
//! is visible. [`StartupTrace::violations`] returns subsystems that started
//! before that marker and are not on the allowlist, which is the CI gate for
//! the "zero work before first paint for non-core features" budget.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const SUBSYSTEM_PREFIX: &str = "subsystem.";
pub const FIRST_PAINT: &str = "first_paint";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Begin,
    End,
    Mark,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub name: String,
    pub kind: EventKind,
    /// Time since the trace was created.
    pub at: Duration,
}

#[derive(Debug)]
pub struct StartupTrace {
    origin: Instant,
    events: Mutex<Vec<Event>>,
}

impl Default for StartupTrace {
    fn default() -> Self {
        Self::new()
    }
}

impl StartupTrace {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
            events: Mutex::new(Vec::new()),
        }
    }

    /// Process-wide trace, created on first use. Call [`global`] early in `main`.
    pub fn global() -> &'static StartupTrace {
        global()
    }

    fn push(&self, name: &str, kind: EventKind) {
        let at = self.origin.elapsed();
        if let Ok(mut ev) = self.events.lock() {
            ev.push(Event {
                name: name.to_owned(),
                kind,
                at,
            });
        }
    }

    /// Open a span; it ends when the guard drops.
    pub fn span(&self, name: &str) -> SpanGuard<'_> {
        self.push(name, EventKind::Begin);
        SpanGuard {
            trace: self,
            name: name.to_owned(),
        }
    }

    pub fn mark(&self, name: &str) {
        self.push(name, EventKind::Mark);
    }

    pub fn mark_first_paint(&self) {
        self.mark(FIRST_PAINT);
    }

    pub fn events(&self) -> Vec<Event> {
        self.events.lock().map(|e| e.clone()).unwrap_or_default()
    }

    pub fn first_paint_at(&self) -> Option<Duration> {
        self.events()
            .iter()
            .find(|e| e.kind == EventKind::Mark && e.name == FIRST_PAINT)
            .map(|e| e.at)
    }

    /// Names of `subsystem.*` spans begun before the first-paint marker (all of
    /// them if the marker has not been recorded), in start order, without the
    /// prefix and deduplicated.
    pub fn subsystems_before_first_paint(&self) -> Vec<String> {
        let events = self.events();
        let cutoff = events
            .iter()
            .position(|e| e.kind == EventKind::Mark && e.name == FIRST_PAINT)
            .unwrap_or(events.len());
        let mut out: Vec<String> = Vec::new();
        for e in &events[..cutoff] {
            if e.kind == EventKind::Begin
                && let Some(rest) = e.name.strip_prefix(SUBSYSTEM_PREFIX)
                && !out.iter().any(|o| o == rest)
            {
                out.push(rest.to_owned());
            }
        }
        out
    }

    /// Subsystems initialized before first paint that are not allowed.
    /// Allowlist entries are subsystem names, with or without the
    /// `subsystem.` prefix; an entry ending in `.*` allows a whole family.
    pub fn violations(&self, allowlist: &[&str]) -> Vec<String> {
        self.subsystems_before_first_paint()
            .into_iter()
            .filter(|s| !is_allowed(s, allowlist))
            .collect()
    }

    /// Compact text dump for logs and CI artifacts.
    pub fn report(&self) -> String {
        let mut s = String::new();
        for e in self.events() {
            let tag = match e.kind {
                EventKind::Begin => "begin",
                EventKind::End => "end  ",
                EventKind::Mark => "mark ",
            };
            s.push_str(&format!(
                "{:>9.3} ms  {tag}  {}\n",
                e.at.as_secs_f64() * 1e3,
                e.name
            ));
        }
        s
    }
}

fn is_allowed(sub: &str, allowlist: &[&str]) -> bool {
    allowlist.iter().any(|a| {
        let a = a.strip_prefix(SUBSYSTEM_PREFIX).unwrap_or(a);
        match a.strip_suffix(".*") {
            Some(family) => sub == family || sub.starts_with(&format!("{family}.")),
            None => a == sub,
        }
    })
}

pub struct SpanGuard<'a> {
    trace: &'a StartupTrace,
    name: String,
}

impl Drop for SpanGuard<'_> {
    fn drop(&mut self) {
        self.trace.push(&self.name, EventKind::End);
    }
}

pub fn global() -> &'static StartupTrace {
    static T: OnceLock<StartupTrace> = OnceLock::new();
    T.get_or_init(StartupTrace::new)
}

/// Open a span on the global trace.
pub fn span(name: &str) -> SpanGuard<'static> {
    global().span(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_disallowed_subsystems_before_first_paint() {
        let t = StartupTrace::new();
        {
            let _g = t.span("subsystem.window");
        }
        {
            let _g = t.span("subsystem.pdf_engine");
        }
        {
            let _g = t.span("subsystem.ocr");
        }
        {
            let _g = t.span("not_a_subsystem");
        }
        t.mark_first_paint();
        {
            let _g = t.span("subsystem.spellcheck"); // after paint: fine
        }
        assert_eq!(
            t.subsystems_before_first_paint(),
            vec!["window", "pdf_engine", "ocr"]
        );
        assert_eq!(
            t.violations(&["window", "subsystem.pdf_engine"]),
            vec!["ocr"]
        );
        assert!(t.violations(&["window", "pdf_engine", "ocr"]).is_empty());
    }

    #[test]
    fn wildcard_and_dedup() {
        let t = StartupTrace::new();
        for n in [
            "subsystem.ui.shell",
            "subsystem.ui.menu",
            "subsystem.ui.shell",
        ] {
            let _g = t.span(n);
        }
        t.mark_first_paint();
        assert_eq!(t.subsystems_before_first_paint().len(), 2);
        assert!(t.violations(&["ui.*"]).is_empty());
        assert_eq!(t.violations(&["ui.shell"]), vec!["ui.menu"]);
    }

    #[test]
    fn nested_spans_record_end_and_timestamps_increase() {
        let t = StartupTrace::new();
        {
            let _outer = t.span("subsystem.a");
            let _inner = t.span("subsystem.b");
        }
        let ev = t.events();
        assert_eq!(ev.len(), 4);
        assert_eq!(ev[2].kind, EventKind::End);
        assert_eq!(ev[2].name, "subsystem.b");
        assert!(ev.windows(2).all(|w| w[0].at <= w[1].at));
        assert!(t.report().contains("subsystem.a"));
    }

    #[test]
    fn no_marker_means_everything_counts() {
        let t = StartupTrace::new();
        let _g = t.span("subsystem.x");
        assert_eq!(t.violations(&[]), vec!["x"]);
        assert!(t.first_paint_at().is_none());
    }

    #[test]
    fn global_is_shared() {
        let _g = span("subsystem.global_test");
        assert!(
            global()
                .events()
                .iter()
                .any(|e| e.name == "subsystem.global_test")
        );
    }
}
