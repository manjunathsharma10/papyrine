//! Child-side request loop.
//!
//! Two threads: an IPC reader that parses frames, registers job cancel tokens
//! and flips them the moment `Cancel` arrives (even while the worker is busy),
//! and the calling thread, which runs the [`Handler`] one request at a time.
//! The handler is built on the calling thread, so it may own `!Send` state
//! (PDFium, a qpdf document).

use crate::proto::{
    ChildToHost, ErrorCode, Hello, HostToChild, IpcError, JobId, PROTOCOL_MIN, PROTOCOL_VERSION,
    Progress, RequestId, Role, negotiate,
};
use crate::transport::{Endpoint, Receiver, Sender, TransportError};
use papyrine_core::CancelToken;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

pub trait Handler {
    type Req: DeserializeOwned + Send + 'static;
    type Resp: Serialize + Send + 'static;

    fn handle(
        &mut self,
        cx: &mut Ctx<'_, Self::Resp>,
        req: Self::Req,
    ) -> Result<Self::Resp, IpcError>;
}

/// Per-request context: identity, cancellation and progress reporting.
pub struct Ctx<'a, R> {
    pub id: RequestId,
    pub job: Option<JobId>,
    cancel: CancelToken,
    tx: &'a Sender,
    _r: PhantomData<fn(R)>,
}

impl<R: Serialize> Ctx<'_, R> {
    pub fn cancel_token(&self) -> &CancelToken {
        &self.cancel
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// `Err(Cancelled)` once the host cancelled this job.
    pub fn check_cancelled(&self) -> Result<(), IpcError> {
        if self.cancel.is_cancelled() {
            Err(IpcError::cancelled())
        } else {
            Ok(())
        }
    }

    /// Report progress (no-op for requests that are not jobs).
    pub fn progress(&self, done: u64, total: u64, message: impl Into<String>) {
        if let Some(job) = self.job {
            let _ = self.tx.send(&ChildToHost::<R>::Progress(Progress {
                job,
                done,
                total,
                message: message.into(),
            }));
        }
    }

    /// Send an intermediate result of the running job.
    pub fn partial(&self, value: R) {
        if let Some(job) = self.job {
            let _ = self.tx.send(&ChildToHost::Partial { job, value });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeExit {
    /// The host asked us to stop.
    Shutdown,
    /// The host closed the connection (crash or quit).
    HostGone,
}

#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error("host did not open with Hello")]
    NoHello,
    #[error("no common protocol version (host speaks {0}..={1})")]
    VersionMismatch(u16, u16),
}

struct Work<Q> {
    id: RequestId,
    job: Option<JobId>,
    body: Q,
    cancel: CancelToken,
}

/// Run the child's loop until shutdown or host disconnect.
pub fn serve<H: Handler>(
    endpoint: Endpoint,
    role: Role,
    make: impl FnOnce() -> H,
) -> Result<ServeExit, ServeError> {
    let Endpoint { tx, mut rx } = endpoint;
    let tx = Arc::new(tx);

    // Handshake: the host speaks first.
    let hello = match rx.recv::<HostToChild<H::Req>>()? {
        Some(HostToChild::Hello(h)) => h,
        _ => return Err(ServeError::NoHello),
    };
    let ours = Hello::new(role);
    tx.send(&ChildToHost::<H::Resp>::Hello(ours))?;
    if negotiate((PROTOCOL_MIN, PROTOCOL_VERSION), (hello.min, hello.max)).is_none() {
        return Err(ServeError::VersionMismatch(hello.min, hello.max));
    }

    let jobs: Arc<Mutex<HashMap<JobId, CancelToken>>> = Arc::default();
    let (work_tx, work_rx) = mpsc::channel::<Work<H::Req>>();
    let exit = Arc::new(Mutex::new(ServeExit::HostGone));

    let reader = {
        let jobs = jobs.clone();
        let exit = exit.clone();
        std::thread::Builder::new()
            .name("papyrine-ipc-reader".into())
            .spawn(move || reader_loop::<H::Req>(rx, work_tx, jobs, exit))
            .map_err(TransportError::Io)?
    };

    let mut handler = make();
    while let Ok(w) = work_rx.recv() {
        let mut cx = Ctx::<H::Resp> {
            id: w.id,
            job: w.job,
            cancel: w.cancel,
            tx: &tx,
            _r: PhantomData,
        };
        let result = if cx.is_cancelled() {
            Err(IpcError::cancelled())
        } else {
            match catch_unwind(AssertUnwindSafe(|| handler.handle(&mut cx, w.body))) {
                Ok(r) => r,
                Err(p) => Err(IpcError::new(ErrorCode::Internal, panic_message(&*p))),
            }
        };
        if let Some(job) = w.job {
            jobs.lock().unwrap_or_else(|p| p.into_inner()).remove(&job);
        }
        if tx
            .send(&ChildToHost::<H::Resp>::Response { id: w.id, result })
            .is_err()
        {
            break;
        }
    }
    drop(reader); // detached: it exits when the socket closes
    let e = *exit.lock().unwrap_or_else(|p| p.into_inner());
    Ok(e)
}

fn reader_loop<Q: DeserializeOwned>(
    mut rx: Receiver,
    work: mpsc::Sender<Work<Q>>,
    jobs: Arc<Mutex<HashMap<JobId, CancelToken>>>,
    exit: Arc<Mutex<ServeExit>>,
) {
    loop {
        match rx.recv::<HostToChild<Q>>() {
            Ok(Some(HostToChild::Request { id, job, body })) => {
                let cancel = CancelToken::new();
                if let Some(j) = job {
                    jobs.lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .insert(j, cancel.clone());
                }
                if work
                    .send(Work {
                        id,
                        job,
                        body,
                        cancel,
                    })
                    .is_err()
                {
                    return;
                }
            }
            Ok(Some(HostToChild::Cancel { job })) => {
                if let Some(t) = jobs.lock().unwrap_or_else(|p| p.into_inner()).get(&job) {
                    t.cancel();
                }
            }
            Ok(Some(HostToChild::Hello(_))) => {} // duplicate hello: ignore
            Ok(Some(HostToChild::Shutdown)) => {
                *exit.lock().unwrap_or_else(|p| p.into_inner()) = ServeExit::Shutdown;
                return;
            }
            Ok(None) | Err(_) => return, // closing `work` ends the loop
        }
    }
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        format!("panic: {s}")
    } else if let Some(s) = p.downcast_ref::<String>() {
        format!("panic: {s}")
    } else {
        "panic".into()
    }
}
