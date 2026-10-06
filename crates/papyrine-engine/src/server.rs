//! The engine role's request loop: routes [`Request`]s to document actors.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use papyrine_core::CancelToken;
use papyrine_ipc::{
    Ctx, DocId, Endpoint, ErrorCode, Handler, IpcError, Role, ServeError, ServeExit, serve,
};
use papyrine_ops::CommandRegistry;

use crate::actor::Actor;
use crate::proto::*;
use crate::registry::default_registry;
use crate::state::{DocState, SaveReply};
use crate::util::scratch_dir;

pub struct EngineHandler {
    docs: HashMap<DocId, Actor>,
    registry: Arc<CommandRegistry>,
    scratch: PathBuf,
}

impl Default for EngineHandler {
    fn default() -> Self {
        Self::new(default_registry())
    }
}

impl EngineHandler {
    pub fn new(registry: CommandRegistry) -> Self {
        Self::with_scratch(registry, scratch_dir())
    }

    pub fn with_scratch(registry: CommandRegistry, scratch: PathBuf) -> Self {
        EngineHandler {
            docs: HashMap::new(),
            registry: Arc::new(registry),
            scratch,
        }
    }

    pub fn open_documents(&self) -> usize {
        self.docs.len()
    }

    fn actor(&self, doc: DocId) -> Result<&Actor> {
        self.docs.get(&doc).ok_or(EngineError::NoDoc)
    }

    /// Serve one request. `cancel` is the job's token (a fresh one for plain requests).
    pub fn dispatch(
        &mut self,
        req: Request,
        cancel: &CancelToken,
    ) -> std::result::Result<Response, IpcError> {
        self.dispatch_inner(req, cancel).map_err(Into::into)
    }

    fn dispatch_inner(&mut self, req: Request, cancel: &CancelToken) -> Result<Response> {
        Ok(match req {
            Request::Ping { nonce } => Response::Pong { nonce },
            Request::Open {
                doc,
                source,
                password,
                params,
            } => {
                if self.docs.contains_key(&doc) {
                    return Err(EngineError::Invalid(format!(
                        "document {} is already open",
                        doc.0
                    )));
                }
                let bytes = source.into_bytes()?;
                let registry = self.registry.clone();
                let scratch = self.scratch.clone();
                let (actor, summary) = Actor::spawn(move || {
                    DocState::open(doc, bytes, password, registry, &params, scratch)
                })?;
                self.docs.insert(doc, actor);
                Response::Opened(Box::new(summary))
            }
            Request::Close { doc } => {
                self.docs.remove(&doc).ok_or(EngineError::NoDoc)?;
                Response::Closed
            }
            Request::Execute {
                doc,
                command,
                blobs,
                out_section,
            } => Response::Committed(Box::new(
                self.actor(doc)?
                    .call(move |st| st.execute(&command, &blobs, out_section.as_ref()))??,
            )),
            Request::Undo { doc, out_section } => Response::Committed(Box::new(
                self.actor(doc)?
                    .call(move |st| st.undo(out_section.as_ref()))??,
            )),
            Request::Redo { doc, out_section } => Response::Committed(Box::new(
                self.actor(doc)?
                    .call(move |st| st.redo(out_section.as_ref()))??,
            )),
            Request::Replay {
                doc,
                commits,
                out_section,
            } => Response::Replayed(Box::new(
                self.actor(doc)?
                    .call(move |st| st.replay(&commits, out_section.as_ref()))??,
            )),
            Request::Save { doc, mode } => {
                let c = cancel.clone();
                match self.actor(doc)?.call(move |st| st.save(&mode, &c))?? {
                    SaveReply::Ready(p) => Response::SaveReady(Box::new(p)),
                    SaveReply::Decision(d) => Response::SaveDecision(d),
                }
            }
            Request::SaveCommitted { doc, token, saved } => {
                let bytes = saved.map(|s| s.into_bytes()).transpose()?;
                Response::SaveFinished(
                    self.actor(doc)?
                        .call(move |st| st.save_committed(token, bytes))??,
                )
            }
            Request::Checkpoint { doc } => {
                let c = cancel.clone();
                Response::Checkpointed(self.actor(doc)?.call(move |st| st.checkpoint(&c))??)
            }
            Request::Compact { doc, level } => {
                let c = cancel.clone();
                Response::Compacted(self.actor(doc)?.call(move |st| st.compact(level, &c))??)
            }
            Request::Query { doc, query } => Response::Query(Box::new(
                self.actor(doc)?.call(move |st| st.query(&query))??,
            )),
            Request::Digest { doc, per_object } => {
                Response::Digest(self.actor(doc)?.call(move |st| st.digest(per_object))??)
            }
        })
    }
}

impl Handler for EngineHandler {
    type Req = Request;
    type Resp = Response;

    fn handle(
        &mut self,
        cx: &mut Ctx<'_, Response>,
        req: Request,
    ) -> std::result::Result<Response, IpcError> {
        let token = cx.cancel_token().clone();
        let r = self.dispatch(req, &token);
        if token.is_cancelled() && r.is_err() {
            return Err(IpcError::new(ErrorCode::Cancelled, "cancelled"));
        }
        r
    }
}

/// Run the engine role over `endpoint` until the host shuts it down or disappears.
pub fn run_engine(endpoint: Endpoint) -> std::result::Result<ServeExit, ServeError> {
    serve(endpoint, Role::Engine, EngineHandler::default)
}
