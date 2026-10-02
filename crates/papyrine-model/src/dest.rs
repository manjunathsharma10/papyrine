//! Destinations (explicit and named) and actions.

use std::collections::HashSet;
use std::rc::Rc;

use papyrine_core::geom::Rect;
use papyrine_cos::{ObjId, Object, ObjectKind, Result};

use crate::model::{Key, MAX_DEPTH, Model};

/// How the target page is shown.
#[derive(Debug, Clone, PartialEq)]
pub enum Fit {
    Xyz {
        left: Option<f64>,
        top: Option<f64>,
        zoom: Option<f64>,
    },
    Fit,
    FitH {
        top: Option<f64>,
    },
    FitV {
        left: Option<f64>,
    },
    FitR(Rect),
    FitB,
    FitBH {
        top: Option<f64>,
    },
    FitBV {
        left: Option<f64>,
    },
    /// A fit name this model does not know (kept verbatim).
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestPage {
    /// A page of this document.
    Local { index: usize, id: ObjId },
    /// A zero-based page number inside another document (GoToR / GoToE).
    Remote(i64),
    /// Named destination that cannot be resolved here, or a reference to a missing page.
    Unresolved,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Destination {
    /// The name this destination was reached through, if it was a named destination.
    pub name: Option<String>,
    pub page: DestPage,
    pub fit: Fit,
}

impl Destination {
    /// Zero-based page index when the destination is in this document.
    pub fn local_page(&self) -> Option<usize> {
        match self.page {
            DestPage::Local { index, .. } => Some(index),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ActionKind {
    GoTo(Destination),
    GoToR {
        file: Option<String>,
        dest: Option<Destination>,
        new_window: Option<bool>,
    },
    GoToE {
        file: Option<String>,
        dest: Option<Destination>,
    },
    Launch {
        file: Option<String>,
        new_window: Option<bool>,
    },
    Uri {
        uri: String,
        is_map: bool,
    },
    /// `NextPage`, `PrevPage`, `FirstPage`, `LastPage`, `Print`, `GoBack`, ...
    Named(String),
    JavaScript(String),
    SubmitForm {
        url: Option<String>,
        flags: u32,
        fields: Vec<String>,
    },
    ResetForm {
        flags: u32,
        fields: Vec<String>,
    },
    ImportData {
        file: Option<String>,
    },
    Hide {
        targets: Vec<String>,
        hide: bool,
    },
    SetOcgState,
    Thread,
    Sound,
    Movie,
    Rendition,
    Trans,
    GoTo3DView,
    RichMediaExecute,
    /// An `/S` this model does not know (kept verbatim); also used when `/S` is missing.
    Unknown(String),
}

/// An action and its `/Next` chain.
#[derive(Debug, Clone, PartialEq)]
pub struct Action {
    pub kind: ActionKind,
    pub next: Vec<Action>,
}

/// What a click on an external-reaching action wants to touch; the UI shows a prompt first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalKind {
    Uri,
    Launch,
    GoToR,
    GoToE,
    SubmitForm,
    ImportData,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalTarget {
    pub kind: ExternalKind,
    /// URL or file specification (empty if the action names none).
    pub target: String,
}

impl Action {
    /// Targets that leave the document, across this action and its `/Next` chain.
    pub fn external_targets(&self) -> Vec<ExternalTarget> {
        let mut out = Vec::new();
        self.collect_external(&mut out);
        out
    }

    fn collect_external(&self, out: &mut Vec<ExternalTarget>) {
        let t = |kind, target: &Option<String>| ExternalTarget {
            kind,
            target: target.clone().unwrap_or_default(),
        };
        match &self.kind {
            ActionKind::Uri { uri, .. } => out.push(ExternalTarget {
                kind: ExternalKind::Uri,
                target: uri.clone(),
            }),
            ActionKind::Launch { file, .. } => out.push(t(ExternalKind::Launch, file)),
            ActionKind::GoToR { file, .. } => out.push(t(ExternalKind::GoToR, file)),
            ActionKind::GoToE { file, .. } => out.push(t(ExternalKind::GoToE, file)),
            ActionKind::SubmitForm { url, .. } => out.push(t(ExternalKind::SubmitForm, url)),
            ActionKind::ImportData { file } => out.push(t(ExternalKind::ImportData, file)),
            _ => {}
        }
        for n in &self.next {
            n.collect_external(out);
        }
    }

    /// Does acting on this need a confirmation prompt (leaves the document)?
    pub fn needs_prompt(&self) -> bool {
        !self.external_targets().is_empty()
    }

    /// All JavaScript sources in this action and its `/Next` chain.
    pub fn scripts(&self) -> Vec<&str> {
        let mut out = Vec::new();
        let mut stack = vec![self];
        while let Some(a) = stack.pop() {
            if let ActionKind::JavaScript(s) = &a.kind {
                out.push(s.as_str());
            }
            stack.extend(a.next.iter().rev());
        }
        out
    }

    /// The first destination reachable by a plain GoTo.
    pub fn goto_destination(&self) -> Option<&Destination> {
        match &self.kind {
            ActionKind::GoTo(d) => Some(d),
            _ => None,
        }
    }
}

impl Model {
    // ----- destinations --------------------------------------------------------------------

    /// Resolve a destination value: an explicit array, a name or string (named destination), or
    /// a dictionary with `/D`.
    pub fn parse_destination(&self, o: &Object) -> Option<Destination> {
        self.destination_inner(o, 0)
    }

    fn destination_inner(&self, o: &Object, depth: usize) -> Option<Destination> {
        if depth > 4 {
            return None;
        }
        match self.kind(o) {
            ObjectKind::Array => self.explicit_destination(o),
            ObjectKind::Dictionary => {
                let d = self.get(o, "D")?;
                self.destination_inner(&d, depth + 1)
            }
            ObjectKind::Name | ObjectKind::String => {
                let name_bytes = if self.kind(o) == ObjectKind::Name {
                    o.name().ok()?
                } else {
                    self.bytes(o)?
                };
                let name = if self.kind(o) == ObjectKind::Name {
                    String::from_utf8_lossy(&name_bytes).into_owned()
                } else {
                    crate::text::decode_text_string(&name_bytes)
                };
                match self.named_destination_value(&name_bytes) {
                    Some(v) => {
                        let mut d = self.destination_inner(&v, depth + 1)?;
                        d.name = Some(name);
                        Some(d)
                    }
                    None => Some(Destination {
                        name: Some(name),
                        page: DestPage::Unresolved,
                        fit: Fit::Fit,
                    }),
                }
            }
            _ => None,
        }
    }

    fn explicit_destination(&self, arr: &Object) -> Option<Destination> {
        let items = self.items(arr);
        let first = items.first()?;
        let page = if let Some(n) = self.int(first)
            && self.kind(first) == ObjectKind::Integer
        {
            DestPage::Remote(n)
        } else if self.is_dict(first) {
            let found = first.id().and_then(|id| {
                self.page_list()
                    .ok()
                    .and_then(|l| l.index_of(id))
                    .map(|index| DestPage::Local { index, id })
            });
            found.unwrap_or(DestPage::Unresolved)
        } else {
            DestPage::Unresolved
        };
        let opt_num = |i: usize| items.get(i).and_then(|o| self.num(o));
        let fit_name = items
            .get(1)
            .and_then(|o| self.name(o))
            .unwrap_or_else(|| "Fit".to_string());
        let fit = match fit_name.as_str() {
            "XYZ" => Fit::Xyz {
                left: opt_num(2),
                top: opt_num(3),
                zoom: opt_num(4),
            },
            "Fit" => Fit::Fit,
            "FitH" => Fit::FitH { top: opt_num(2) },
            "FitV" => Fit::FitV { left: opt_num(2) },
            "FitR" => match (opt_num(2), opt_num(3), opt_num(4), opt_num(5)) {
                (Some(a), Some(b), Some(c), Some(d)) => {
                    Fit::FitR(Rect::new(a, b, c, d).normalized())
                }
                _ => Fit::Fit,
            },
            "FitB" => Fit::FitB,
            "FitBH" => Fit::FitBH { top: opt_num(2) },
            "FitBV" => Fit::FitBV { left: opt_num(2) },
            other => Fit::Unknown(other.to_string()),
        };
        Some(Destination {
            name: None,
            page,
            fit,
        })
    }

    /// The raw value (array or `<< /D ... >>`) a named destination maps to: the catalog's
    /// `/Dests` dictionary (PDF 1.1) first, then the `/Names /Dests` name tree.
    pub(crate) fn named_destination_value(&self, name: &[u8]) -> Option<Object> {
        let root = self.root()?;
        if let Some(d) = self.get(&root, "Dests")
            && self.is_dict(&d)
        {
            let key = String::from_utf8_lossy(name);
            if let Some(v) = self.get(&d, &key) {
                return Some(v);
            }
        }
        let names = self.get(&root, "Names")?;
        let tree = self.get(&names, "Dests")?;
        self.name_tree_lookup(&tree, name)
    }

    /// Every named destination (both sources), resolved. Cached.
    pub fn named_destinations(&self) -> Result<Rc<Vec<Destination>>> {
        self.cached(Key::NamedDests, |m| {
            let mut out = Vec::new();
            let Some(root) = m.root() else {
                return Ok(out);
            };
            if let Some(d) = m.get(&root, "Dests")
                && m.is_dict(&d)
            {
                for k in m.dict_keys(&d) {
                    if let Some(v) = m.get(&d, &k)
                        && let Some(mut dest) = m.destination_inner(&v, 1)
                    {
                        dest.name = Some(k);
                        out.push(dest);
                    }
                }
            }
            if let Some(names) = m.get(&root, "Names")
                && let Some(tree) = m.get(&names, "Dests")
            {
                for (k, v) in m.name_tree_entries(&tree) {
                    if let Some(mut dest) = m.destination_inner(&v, 1) {
                        dest.name = Some(crate::text::decode_text_string(&k));
                        out.push(dest);
                    }
                }
            }
            Ok(out)
        })
    }

    // ----- actions -------------------------------------------------------------------------

    /// Parse an action dictionary and its `/Next` chain.
    pub fn parse_action(&self, o: &Object) -> Option<Action> {
        let mut seen = HashSet::new();
        self.action_inner(o, 0, &mut seen)
    }

    fn action_inner(&self, o: &Object, depth: usize, seen: &mut HashSet<ObjId>) -> Option<Action> {
        if depth > MAX_DEPTH / 4 || !self.is_dict(o) {
            return None;
        }
        if let Some(id) = o.id()
            && !seen.insert(id)
        {
            self.note(format!("action /Next cycle at {id}"));
            return None;
        }
        let s = self.name_of(o, "S").unwrap_or_default();
        let kind = match s.as_str() {
            "GoTo" => {
                let d = self
                    .get(o, "D")
                    .and_then(|d| self.parse_destination(&d))
                    .unwrap_or(Destination {
                        name: None,
                        page: DestPage::Unresolved,
                        fit: Fit::Fit,
                    });
                ActionKind::GoTo(d)
            }
            "GoToR" => ActionKind::GoToR {
                file: self.get(o, "F").and_then(|f| self.file_spec(&f)),
                dest: self.get(o, "D").and_then(|d| self.parse_destination(&d)),
                new_window: self.bool_of(o, "NewWindow"),
            },
            "GoToE" => ActionKind::GoToE {
                file: self.get(o, "F").and_then(|f| self.file_spec(&f)),
                dest: self.get(o, "D").and_then(|d| self.parse_destination(&d)),
            },
            "Launch" => ActionKind::Launch {
                file: self.get(o, "F").and_then(|f| self.file_spec(&f)),
                new_window: self.bool_of(o, "NewWindow"),
            },
            "URI" => ActionKind::Uri {
                uri: self
                    .get(o, "URI")
                    .and_then(|u| self.bytes(&u))
                    // URIs are 7-bit ASCII byte strings, not text strings.
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default(),
                is_map: self.bool_of(o, "IsMap").unwrap_or(false),
            },
            "Named" => ActionKind::Named(self.name_of(o, "N").unwrap_or_default()),
            "JavaScript" => ActionKind::JavaScript(
                self.get(o, "JS")
                    .and_then(|j| self.text_or_stream(&j))
                    .unwrap_or_default(),
            ),
            "SubmitForm" => ActionKind::SubmitForm {
                url: self.get(o, "F").and_then(|f| self.file_spec(&f)),
                flags: self.int_of(o, "Flags").unwrap_or(0) as u32,
                fields: self.field_name_list(o),
            },
            "ResetForm" => ActionKind::ResetForm {
                flags: self.int_of(o, "Flags").unwrap_or(0) as u32,
                fields: self.field_name_list(o),
            },
            "ImportData" => ActionKind::ImportData {
                file: self.get(o, "F").and_then(|f| self.file_spec(&f)),
            },
            "Hide" => {
                let mut targets = Vec::new();
                if let Some(t) = self.get(o, "T") {
                    let list = if self.kind(&t) == ObjectKind::Array {
                        self.items(&t)
                    } else {
                        vec![t]
                    };
                    for e in list {
                        if let Some(n) = self.text(&e) {
                            targets.push(n);
                        } else if self.is_dict(&e)
                            && let Some(n) = self.text_of(&e, "T")
                        {
                            targets.push(n);
                        }
                    }
                }
                ActionKind::Hide {
                    targets,
                    hide: self.bool_of(o, "H").unwrap_or(true),
                }
            }
            "SetOCGState" => ActionKind::SetOcgState,
            "Thread" => ActionKind::Thread,
            "Sound" => ActionKind::Sound,
            "Movie" => ActionKind::Movie,
            "Rendition" => ActionKind::Rendition,
            "Trans" => ActionKind::Trans,
            "GoTo3DView" => ActionKind::GoTo3DView,
            "RichMediaExecute" => ActionKind::RichMediaExecute,
            other => ActionKind::Unknown(other.to_string()),
        };
        let mut next = Vec::new();
        if let Some(n) = self.get(o, "Next") {
            let list = if self.kind(&n) == ObjectKind::Array {
                self.items(&n)
            } else {
                vec![n]
            };
            for e in list.iter().take(64) {
                if let Some(a) = self.action_inner(e, depth + 1, seen) {
                    next.push(a);
                }
            }
        }
        Some(Action { kind, next })
    }

    /// A file specification: a string, or a dictionary with `/UF`, `/F`, or the legacy
    /// `/Unix`, `/DOS`, `/Mac` entries (URL file systems yield the URL).
    pub(crate) fn file_spec(&self, o: &Object) -> Option<String> {
        match self.kind(o) {
            ObjectKind::String => self.bytes(o).map(|b| crate::text::decode_text_string(&b)),
            ObjectKind::Dictionary | ObjectKind::Stream => ["UF", "F", "Unix", "DOS", "Mac"]
                .iter()
                .find_map(|k| self.get(o, k).and_then(|v| self.bytes(&v)))
                .map(|b| crate::text::decode_text_string(&b)),
            _ => None,
        }
    }

    fn field_name_list(&self, o: &Object) -> Vec<String> {
        self.items_of(o, "Fields")
            .iter()
            .filter_map(|f| {
                self.text(f).or_else(|| {
                    // A field dictionary reference: use its partial name.
                    self.is_dict(f).then(|| self.text_of(f, "T")).flatten()
                })
            })
            .collect()
    }
}
