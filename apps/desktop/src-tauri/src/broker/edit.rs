//! Commands: journal Intent -> engine -> journal Commit -> renderer snapshot
//! (ARCHITECTURE §5), engine crash handling, checkpoints and the unfinished-action prompt.

use std::sync::Arc;

use papyrine_engine::proto::{Committed, Request, Response};
use papyrine_ipc::{CommandRequest, DocId, SharedRegion};
use papyrine_journal::{
    AfterImage as JAfterImage, BlobRef, Journal, JournalError, ObjId, Resolution, SystemClock,
};
use serde_json::{Value, json};

use super::Broker;
use crate::api::{CommandResult, EngineCommand, HostEvent};
use crate::error::{Code, HostErr, Result, is_crash};
use crate::session::{JournalHandle, Session};
use crate::util::dup_region;

const SECTION_INIT: usize = 1 << 20;

/// Registry name, params, user-facing label.
pub(crate) fn map_command(cmd: &EngineCommand) -> Result<(String, Value, String)> {
    Ok(match cmd {
        EngineCommand::RotatePages { pages, degrees } => (
            "rotate_pages".into(),
            json!({"pages": pages, "delta": degrees}),
            "Rotate pages".into(),
        ),
        EngineCommand::DeletePages { pages } => (
            "delete_pages".into(),
            json!({"pages": pages}),
            "Delete pages".into(),
        ),
        EngineCommand::MovePages { pages, to } => (
            "move_pages".into(),
            json!({"pages": pages, "to": to}),
            "Move pages".into(),
        ),
        EngineCommand::SetMetadata { fields } => {
            let mut cmds = Vec::new();
            for (field, v) in [
                ("Title", &fields.title),
                ("Author", &fields.author),
                ("Subject", &fields.subject),
                ("Keywords", &fields.keywords),
            ] {
                if let Some(v) = v {
                    cmds.push(json!({
                        "name": "set_info_field",
                        "params": {"field": field, "value": if v.is_empty() { Value::Null } else { json!(v) }},
                    }));
                }
            }
            if cmds.is_empty() {
                return Err(HostErr::internal("no document properties to change"));
            }
            (
                "composite".into(),
                json!({"label": "Edit document properties", "commands": cmds}),
                "Edit document properties".into(),
            )
        }
    })
}

/// A human label for the "last action didn't finish" prompt.
pub(crate) fn label_for(command: &str, params: &Value) -> String {
    match command {
        "rotate_pages" => "Rotate pages".into(),
        "delete_pages" => "Delete pages".into(),
        "move_pages" => "Move pages".into(),
        "duplicate_pages" => "Duplicate pages".into(),
        "insert_blank_page" => "Insert blank page".into(),
        "set_page_box" => "Change page size".into(),
        "set_info_field" => "Edit document properties".into(),
        "composite" => params
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("Edit")
            .to_string(),
        "undo" => "Undo".into(),
        "redo" => "Redo".into(),
        other => other.replace('_', " "),
    }
}

/// Protocol after-images -> journal data (first byte: 1 = present, 0 = deleted).
pub(crate) fn to_journal_images(images: &[papyrine_ipc::AfterImage]) -> Vec<JAfterImage> {
    images
        .iter()
        .map(|a| {
            let mut data = Vec::with_capacity(1 + a.body.as_ref().map_or(0, Vec::len));
            match &a.body {
                Some(b) => {
                    data.push(1);
                    data.extend_from_slice(b);
                }
                None => data.push(0),
            }
            JAfterImage {
                id: ObjId {
                    num: a.obj,
                    generation: a.generation,
                },
                data,
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Op {
    Exec { name: String, params: Value },
    Undo,
    Redo,
}

impl Broker {
    pub(crate) fn notice(&self, level: &str, code: &str, message: &str, s: Option<&Session>) {
        self.emit(HostEvent::Notice {
            level: level.into(),
            code: code.into(),
            message: message.into(),
            doc_id: s.map(Session::doc_id),
        });
    }

    // ---------------------------------------------------------------------- journal

    pub(crate) fn ensure_journal(&self, s: &Session) -> Option<Journal> {
        {
            let st = s.st();
            if let Some(j) = &st.journal {
                return Some(j.journal.clone());
            }
            if st.journal_failed {
                return None;
            }
        }
        let base_path = s.st().base_path.clone();
        let created = papyrine_journal::BaseInfo::from_file(&base_path)
            .map_err(|e| e.to_string())
            .and_then(|base| {
                Journal::create(
                    &self.inner.recovery,
                    self.inner.fs.clone(),
                    &s.doc_key,
                    &base,
                    self.inner.cfg.journal.clone(),
                    Arc::new(SystemClock),
                )
                .map_err(|e| e.to_string())
            });
        let mut st = s.st();
        match created {
            Ok(journal) => {
                let flusher = journal.spawn_flusher();
                st.journal = Some(JournalHandle {
                    journal: journal.clone(),
                    _flusher: flusher,
                });
                Some(journal)
            }
            Err(e) => {
                st.journal_failed = true;
                drop(st);
                self.notice(
                    "warn",
                    "journal-unavailable",
                    &format!("Crash recovery is unavailable for this document ({e}). Save often."),
                    Some(s),
                );
                None
            }
        }
    }

    // ------------------------------------------------------------------- commands

    pub fn execute(&self, doc_id: &str, cmd: &EngineCommand) -> Result<CommandResult> {
        let s = self.session(doc_id)?;
        let (name, params, label) = map_command(cmd)?;
        self.apply_op(&s, Op::Exec { name, params }, label)
    }

    pub fn undo(&self, doc_id: &str) -> Result<CommandResult> {
        let s = self.session(doc_id)?;
        if !s.st().can_undo {
            return Ok(self.no_change(&s));
        }
        self.apply_op(&s, Op::Undo, "Undo".into())
    }

    pub fn redo(&self, doc_id: &str) -> Result<CommandResult> {
        let s = self.session(doc_id)?;
        if !s.st().can_redo {
            return Ok(self.no_change(&s));
        }
        self.apply_op(&s, Op::Redo, "Redo".into())
    }

    fn no_change(&self, s: &Session) -> CommandResult {
        CommandResult {
            info: s.info(),
            invalidated_pages: vec![],
        }
    }

    pub(crate) fn apply_op(
        &self,
        s: &Arc<Session>,
        op: Op,
        label: String,
    ) -> Result<CommandResult> {
        let _g = s.lock_cmd();
        if s.st().closed {
            return Err(HostErr::not_found("document is closed"));
        }
        if s.st().unfinished.is_some() {
            return Err(HostErr::internal(
                "A previous action didn't finish. Choose Redo or Skip first.",
            ));
        }
        let (jname, jparams) = match &op {
            Op::Exec { name, params } => (name.clone(), params.clone()),
            Op::Undo => ("undo".to_string(), Value::Null),
            Op::Redo => ("redo".to_string(), Value::Null),
        };
        let journal = self.ensure_journal(s);
        let mut token = match &journal {
            Some(j) => Some(
                j.begin(&jname, jparams.clone(), Vec::<BlobRef>::new())
                    .map_err(|e| match e {
                        JournalError::Quarantined { .. } => HostErr::internal(
                            "This action crashed Papyrine twice, so it is disabled for this document.",
                        ),
                        JournalError::Unresolved { .. } => HostErr::internal(
                            "A previous action didn't finish. Choose Redo or Skip first.",
                        ),
                        other => HostErr::from(other),
                    })?,
            ),
            None => None,
        };
        let seq = token.as_ref().map(|t| t.seq());
        let fail_intent = |token: &mut Option<papyrine_journal::IntentToken>| {
            if let (Some(j), Some(t)) = (&journal, token.take()) {
                let _ = j.fail(t);
            }
        };

        let client = match self.engine_client(s) {
            Ok(c) => c,
            Err(e) => {
                fail_intent(&mut token);
                return Err(e);
            }
        };
        let region = SharedRegion::create(SECTION_INIT)?;
        let out = Some(dup_region(&region)?);
        let doc = DocId(s.id);
        let req = match &op {
            Op::Exec { name, params } => Request::Execute {
                doc,
                command: CommandRequest {
                    name: name.clone(),
                    params_json: params.to_string(),
                },
                blobs: vec![],
                out_section: out,
            },
            Op::Undo => Request::Undo {
                doc,
                out_section: out,
            },
            Op::Redo => Request::Redo {
                doc,
                out_section: out,
            },
        };
        let committed: Box<Committed> = match client.call_blocking(req) {
            Ok(Response::Committed(c)) => c,
            Ok(other) => {
                fail_intent(&mut token);
                return Err(HostErr::internal(format!("engine: unexpected {other:?}")));
            }
            Err(e) if is_crash(&e) => return Err(self.engine_crashed(s, &journal, seq, &label)),
            Err(e) => {
                fail_intent(&mut token);
                return Err(e.into());
            }
        };

        if let (Some(j), Some(t)) = (&journal, token.take()) {
            let created: Vec<ObjId> = committed
                .created
                .iter()
                .map(|(num, generation)| ObjId {
                    num: *num,
                    generation: *generation,
                })
                .collect();
            if let Err(e) = j.commit(t, to_journal_images(&committed.after_images), created) {
                self.notice(
                    "warn",
                    "journal-failed",
                    &format!("Crash recovery stopped working ({e}). Save your work."),
                    Some(s),
                );
            }
        }
        let invalidated = self.finish_op(s, &op, *committed, region)?;
        if let Some(j) = &journal {
            self.maybe_checkpoint(s, j);
        }
        let info = s.info();
        self.emit(HostEvent::DocumentChanged {
            info: info.clone(),
            invalidated_pages: invalidated.clone(),
        });
        Ok(CommandResult {
            info,
            invalidated_pages: invalidated,
        })
    }

    /// The engine died while running a command. Count the crash, restart, report.
    fn engine_crashed(
        &self,
        s: &Arc<Session>,
        journal: &Option<Journal>,
        seq: Option<u64>,
        label: &str,
    ) -> HostErr {
        let mut crashes = 0;
        if let (Some(j), Some(seq)) = (journal, seq) {
            crashes = j.resolve_unfinished(seq, Resolution::Skip).unwrap_or(0);
        }
        // Rebuild right away (journal replay) so the next edit is not slowed by it.
        let rebuilt = self.engine_client(s);
        let mut msg =
            format!("Your last action, {label}, didn't finish. Engine restarted; no changes lost.");
        if crashes >= papyrine_journal::CRASH_LIMIT {
            msg.push_str(" It crashed twice and is disabled for this document.");
        }
        if let Err(e) = rebuilt {
            msg = format!(
                "The engine crashed and could not be restarted: {}",
                e.message
            );
        } else if let Ok(pages) = self.render_reopen_pages(s) {
            s.st().pages = pages;
        }
        self.notice("warn", "engine-crashed", &msg, Some(s));
        HostErr::new(Code::Internal, msg)
    }

    /// Update the session after a committed op and bring the renderer along. Returns the
    /// pages whose tiles must be re-requested.
    fn finish_op(
        &self,
        s: &Arc<Session>,
        op: &Op,
        c: Committed,
        region: SharedRegion,
    ) -> Result<Vec<usize>> {
        let summary = c.summary;
        let (action, advice) = self.apply_snapshot(s, c.snapshot, Some(region))?;
        {
            let mut st = s.st();
            st.can_undo = summary.can_undo;
            st.can_redo = summary.can_redo;
            st.revision += 1;
            st.dirty = true;
            st.epoch += 1;
        }
        self.render_sync(s, action)?;
        let touches_meta = match op {
            Op::Exec { name, .. } => name == "composite" || name == "set_info_field",
            Op::Undo | Op::Redo => true,
        };
        if touches_meta {
            self.refresh_meta(s);
        }
        let count = s.st().pages.len();
        let invalidated: Vec<usize> = if summary.structure_changed {
            self.invalidate_doc(s.id);
            self.refresh_labels(s);
            s.st().apply_labels();
            (0..count).collect()
        } else {
            let p: Vec<u32> = summary.dirty_pages.clone();
            self.invalidate_pages(s.id, &p);
            p.iter().map(|x| *x as usize).collect()
        };
        if let Some(level) = advice {
            self.spawn_compaction(s, level);
        }
        Ok(invalidated)
    }

    fn maybe_checkpoint(&self, s: &Arc<Session>, j: &Journal) {
        if !j.checkpoint_due() || s.st().checkpoint_disabled {
            return;
        }
        let b = self.clone();
        let s = s.clone();
        let j = j.clone();
        let _ = std::thread::Builder::new()
            .name("papyrine-checkpoint".into())
            .spawn(move || {
                let _g = s.lock_cmd();
                if s.st().closed {
                    return;
                }
                let doc = DocId(s.id);
                let r = b
                    .with_engine(&s, |c| c.call_blocking(Request::Checkpoint { doc }))
                    .and_then(|resp| match resp {
                        Response::Checkpointed(f) => b.take_output_bytes(&f),
                        other => Err(HostErr::internal(format!("engine: unexpected {other:?}"))),
                    })
                    .and_then(|bytes| j.checkpoint(&bytes).map_err(HostErr::from));
                if let Err(e) = r {
                    // The journal keeps growing; recovery still works from the original.
                    s.st().checkpoint_disabled = true;
                    b.notice(
                        "warn",
                        "checkpoint-failed",
                        &format!("Journal checkpoint failed: {}", e.message),
                        Some(&s),
                    );
                }
            });
    }

    /// Redo or skip the command that was in flight when the last process died.
    pub fn resolve_unfinished(&self, doc_id: &str, redo: bool) -> Result<CommandResult> {
        let s = self.session(doc_id)?;
        let Some(u) = s.st().unfinished.clone() else {
            return Ok(self.no_change(&s));
        };
        let journal = s
            .st()
            .journal
            .as_ref()
            .map(|j| j.journal.clone())
            .ok_or_else(|| HostErr::internal("no journal for this document"))?;
        let params = {
            let _g = s.lock_cmd();
            let how = if redo {
                Resolution::Redo
            } else {
                Resolution::Skip
            };
            journal.resolve_unfinished(u.seq, how)?;
            let mut st = s.st();
            st.unfinished = None;
            st.unfinished_params.take()
        };
        if !redo {
            return Ok(self.no_change(&s));
        }
        self.apply_op(
            &s,
            Op::Exec {
                name: u.command.clone(),
                params: params.unwrap_or(Value::Null),
            },
            u.label.clone(),
        )
    }
}
