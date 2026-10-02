//! JavaScript discovery: where scripts live, without running them.

use std::rc::Rc;

use papyrine_cos::Result;

use crate::dest::{Action, ActionKind};
use crate::model::{Key, Model};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptLocation {
    /// `/Names /JavaScript` entry with this name.
    Document {
        name: String,
    },
    OpenAction,
    /// Catalog `/AA` trigger (`WC WS DS WP DP`).
    Catalog {
        trigger: String,
    },
    Page {
        page: usize,
        trigger: String,
    },
    /// Field additional action (`K F V C` ...).
    Field {
        field: String,
        trigger: String,
    },
    /// A widget's own `/A` action.
    Widget {
        field: String,
    },
}

#[derive(Debug, Clone)]
pub struct Script {
    pub location: ScriptLocation,
    pub source: String,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptInventory {
    pub scripts: Vec<Script>,
}

impl ScriptInventory {
    pub fn has_javascript(&self) -> bool {
        !self.scripts.is_empty()
    }
    pub fn document_level(&self) -> impl Iterator<Item = &Script> {
        self.scripts
            .iter()
            .filter(|s| matches!(s.location, ScriptLocation::Document { .. }))
    }
    pub fn field_scripts(&self) -> impl Iterator<Item = &Script> {
        self.scripts
            .iter()
            .filter(|s| matches!(s.location, ScriptLocation::Field { .. }))
    }
}

impl Model {
    /// All JavaScript in the document-level name tree, OpenAction, catalog and page triggers,
    /// and form fields. Link and screen annotation scripts are not scanned.
    pub fn scripts(&self) -> Result<Rc<ScriptInventory>> {
        self.cached(Key::JavaScript, |m| {
            let mut inv = ScriptInventory::default();
            let mut add = |loc: ScriptLocation, a: &Action| {
                for s in a.scripts() {
                    inv.scripts.push(Script {
                        location: loc.clone(),
                        source: s.to_string(),
                    });
                }
            };
            if let Some(root) = m.root()
                && let Some(names) = m.get(&root, "Names")
                && let Some(tree) = m.get(&names, "JavaScript")
            {
                for (k, v) in m.name_tree_entries(&tree) {
                    if let Some(a) = m.parse_action(&v) {
                        add(
                            ScriptLocation::Document {
                                name: crate::text::decode_text_string(&k),
                            },
                            &a,
                        );
                    }
                }
            }
            let cat = m.catalog_info()?;
            if let Some(crate::docinfo::OpenAction::Action(a)) = &cat.open_action {
                add(ScriptLocation::OpenAction, a);
            }
            for (t, a) in &cat.catalog_actions {
                add(ScriptLocation::Catalog { trigger: t.clone() }, a);
            }
            let n = m.page_count().unwrap_or(0);
            for i in 0..n {
                let Ok(page) = m.page_object(i) else { continue };
                let Some(aa) = m.get(&page, "AA") else {
                    continue;
                };
                for t in ["O", "C"] {
                    if let Some(a) = m.get(&aa, t).and_then(|a| m.parse_action(&a)) {
                        add(
                            ScriptLocation::Page {
                                page: i,
                                trigger: t.into(),
                            },
                            &a,
                        );
                    }
                }
            }
            let form = m.form()?;
            for f in form.terminal_fields() {
                for fa in &f.actions {
                    add(
                        ScriptLocation::Field {
                            field: f.qualified_name.clone(),
                            trigger: fa.trigger.clone(),
                        },
                        &fa.action,
                    );
                }
                for w in &f.widgets {
                    if let Some(a) = &w.action {
                        add(
                            ScriptLocation::Widget {
                                field: f.qualified_name.clone(),
                            },
                            a,
                        );
                    }
                }
            }
            Ok(inv)
        })
    }

    /// Cheap check for the "this document contains scripts" banner.
    pub fn has_javascript(&self) -> Result<bool> {
        Ok(self.scripts()?.has_javascript())
    }
}

/// True when the action (or a chained one) is JavaScript.
pub fn is_javascript(a: &Action) -> bool {
    matches!(a.kind, ActionKind::JavaScript(_)) || a.next.iter().any(is_javascript)
}
