//! Host-side client: sends requests, correlates responses by [`RequestId`],
//! fans job progress and partial results out to their [`Job`].

use crate::proto::{
    ChildToHost, ErrorCode, Hello, HostToChild, IpcError, JobId, PROTOCOL_MIN, PROTOCOL_VERSION,
    Progress, RequestId, Role, negotiate,
};
use crate::transport::{Endpoint, Receiver, Sender};
use papyrine_core::IdGen;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Reply<R> = mpsc::Sender<Result<R, IpcError>>;

struct JobSink<R> {
    progress: mpsc::Sender<Progress>,
    partial: mpsc::Sender<R>,
}

struct Shared<Q, R> {
    tx: Sender,
    ids: IdGen,
    pending: Mutex<HashMap<RequestId, Reply<R>>>,
    jobs: Mutex<HashMap<JobId, JobSink<R>>>,
    closed: Mutex<Option<String>>,
    _q: std::marker::PhantomData<fn(Q)>,
}

/// A connection to one child. Cheap to clone.
pub struct Client<Q, R> {
    shared: Arc<Shared<Q, R>>,
    pub peer: Hello,
    pub version: u16,
}

impl<Q, R> Clone for Client<Q, R> {
    fn clone(&self) -> Self {
        Client {
            shared: self.shared.clone(),
            peer: self.peer.clone(),
            version: self.version,
        }
    }
}

/// An in-flight request.
pub struct Pending<R> {
    rx: mpsc::Receiver<Result<R, IpcError>>,
    pub id: RequestId,
}

impl<R> Pending<R> {
    pub fn wait(self) -> Result<R, IpcError> {
        self.rx
            .recv()
            .unwrap_or_else(|_| Err(crashed("reply channel dropped")))
    }

    pub fn wait_timeout(&self, d: Duration) -> Option<Result<R, IpcError>> {
        match self.rx.recv_timeout(d) {
            Ok(r) => Some(r),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Err(crashed("reply channel dropped"))),
        }
    }
}

/// A cancellable long-running request.
pub struct Job<Q, R> {
    pub id: JobId,
    pub progress: mpsc::Receiver<Progress>,
    pub partials: mpsc::Receiver<R>,
    pending: Pending<R>,
    client: Client<Q, R>,
}

impl<Q: Serialize, R: DeserializeOwned + Send + 'static> Job<Q, R> {
    /// Ask the child to stop. The job still ends with a response, normally
    /// `ErrorCode::Cancelled`; a job that already finished is unaffected.
    pub fn cancel(&self) {
        let _ = self
            .client
            .shared
            .tx
            .send(&HostToChild::<Q>::Cancel { job: self.id });
    }

    pub fn wait(self) -> Result<R, IpcError> {
        let r = self.pending.wait();
        self.client
            .shared
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&self.id);
        r
    }

    pub fn wait_timeout(&self, d: Duration) -> Option<Result<R, IpcError>> {
        self.pending.wait_timeout(d)
    }
}

fn crashed(why: &str) -> IpcError {
    IpcError::new(ErrorCode::ChildCrashed, why)
}

impl<Q, R> Client<Q, R>
where
    Q: Serialize + Send + 'static,
    R: DeserializeOwned + Send + 'static,
{
    /// Handshake with the child behind `endpoint`, then start the reader thread.
    pub fn connect(endpoint: Endpoint, expect: &Role, timeout: Duration) -> Result<Self, IpcError> {
        let Endpoint { tx, rx } = endpoint;
        tx.send(&HostToChild::<Q>::Hello(Hello::new(expect.clone())))
            .map_err(|e| crashed(&format!("hello: {e}")))?;

        let shared = Arc::new(Shared {
            tx,
            ids: IdGen::new(),
            pending: Mutex::default(),
            jobs: Mutex::default(),
            closed: Mutex::default(),
            _q: std::marker::PhantomData,
        });
        let (ready_tx, ready_rx) = mpsc::channel::<Result<Hello, IpcError>>();
        let s2 = shared.clone();
        std::thread::Builder::new()
            .name("papyrine-ipc-client".into())
            .spawn(move || reader_loop(rx, s2, ready_tx))
            .map_err(|e| IpcError::internal(e.to_string()))?;

        let peer = match ready_rx.recv_timeout(timeout) {
            Ok(r) => r?,
            Err(_) => return Err(crashed("child did not complete the handshake in time")),
        };
        let version = negotiate((PROTOCOL_MIN, PROTOCOL_VERSION), (peer.min, peer.max))
            .ok_or_else(|| {
                IpcError::new(
                    ErrorCode::VersionMismatch,
                    format!(
                        "child speaks {}..={}, host {}..={}",
                        peer.min, peer.max, PROTOCOL_MIN, PROTOCOL_VERSION
                    ),
                )
            })?;
        if &peer.role != expect {
            return Err(IpcError::invalid(format!(
                "expected {expect:?}, child is {:?}",
                peer.role
            )));
        }
        Ok(Client {
            shared,
            peer,
            version,
        })
    }

    fn send_request(&self, job: Option<JobId>, body: Q) -> Result<Pending<R>, IpcError> {
        if let Some(why) = self
            .shared
            .closed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
        {
            return Err(crashed(&why));
        }
        let id = self.shared.ids.next_request().into();
        let (tx, rx) = mpsc::channel();
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, tx);
        if let Err(e) = self.shared.tx.send(&HostToChild::Request { id, job, body }) {
            self.shared
                .pending
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&id);
            return Err(crashed(&format!("send failed: {e}")));
        }
        Ok(Pending { rx, id })
    }

    /// Send a request and return immediately.
    pub fn call(&self, body: Q) -> Result<Pending<R>, IpcError> {
        self.send_request(None, body)
    }

    /// Send a request and wait for its response.
    pub fn call_blocking(&self, body: Q) -> Result<R, IpcError> {
        self.call(body)?.wait()
    }

    /// Start a cancellable job with progress and partial results.
    pub fn start_job(&self, body: Q) -> Result<Job<Q, R>, IpcError> {
        let job: JobId = self.shared.ids.next_job().into();
        let (ptx, prx) = mpsc::channel();
        let (qtx, qrx) = mpsc::channel();
        self.shared
            .jobs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                job,
                JobSink {
                    progress: ptx,
                    partial: qtx,
                },
            );
        match self.send_request(Some(job), body) {
            Ok(pending) => Ok(Job {
                id: job,
                progress: prx,
                partials: qrx,
                pending,
                client: self.clone(),
            }),
            Err(e) => {
                self.shared
                    .jobs
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&job);
                Err(e)
            }
        }
    }

    /// Ask the child to exit after its current request.
    pub fn shutdown(&self) {
        let _ = self.shared.tx.send(&HostToChild::<Q>::Shutdown);
    }

    /// Why the connection is closed, if it is.
    pub fn closed_reason(&self) -> Option<String> {
        self.shared
            .closed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

fn reader_loop<Q, R: DeserializeOwned>(
    mut rx: Receiver,
    shared: Arc<Shared<Q, R>>,
    ready: mpsc::Sender<Result<Hello, IpcError>>,
) {
    let mut ready = Some(ready);
    let why = loop {
        match rx.recv::<ChildToHost<R>>() {
            Ok(Some(ChildToHost::Hello(h))) => {
                if let Some(r) = ready.take() {
                    let _ = r.send(Ok(h));
                }
            }
            Ok(Some(ChildToHost::Response { id, result })) => {
                let reply = shared
                    .pending
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&id);
                if let Some(reply) = reply {
                    let _ = reply.send(result);
                }
            }
            Ok(Some(ChildToHost::Progress(p))) => {
                if let Some(s) = shared
                    .jobs
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(&p.job)
                {
                    let _ = s.progress.send(p);
                }
            }
            Ok(Some(ChildToHost::Partial { job, value })) => {
                if let Some(s) = shared
                    .jobs
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .get(&job)
                {
                    let _ = s.partial.send(value);
                }
            }
            Ok(None) => break "child closed the connection".to_string(),
            Err(e) => break format!("connection error: {e}"),
        }
    };
    *shared.closed.lock().unwrap_or_else(|p| p.into_inner()) = Some(why.clone());
    if let Some(r) = ready.take() {
        let _ = r.send(Err(crashed(&why)));
    }
    let pending: Vec<_> = shared
        .pending
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .drain()
        .collect();
    for (_, reply) in pending {
        let _ = reply.send(Err(crashed(&why)));
    }
}
