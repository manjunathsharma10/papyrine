//! Dev bridge (cargo feature `dev-bridge`, never in release builds): a localhost WebSocket
//! server exposing the host contract so headless Chromium (Playwright) can drive the real
//! UI against the real engine and renderer without any visible window.
//!
//! Safety: binds 127.0.0.1 only, requires the random token in the URL path and a
//! localhost `Origin`. It can open any file the user can read, which is why it is a
//! development feature.
//!
//! Wire protocol (text frames are JSON):
//!   request   `{"id":N,"method":"openDocument","args":[...]}`
//!   response  `{"id":N,"ok":true,"result":...}` or `{"id":N,"ok":false,"error":{"code","message","detail"?}}`
//!   event     `{"event":{"type":...}}`
//!   `getTile` / `getPreview` answer with a binary frame: u32 id, u32 width, u32 height, RGBA.
//!   Binary request `[u32 id][u32 name_len][name][pdf bytes]` is `openBytes`.
//!   `dev.*` methods: killEngine, killRenderer, queueOpen, queueSave, stats, shutdown.

use std::collections::VecDeque;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::StatusCode;
use tungstenite::{Message, WebSocket};

use crate::api::{EventSink, HostEvent};
use crate::broker::{Broker, Dialogs};
use crate::dispatch;
use crate::error::HostErr;
use crate::sched::PRIO_VISIBLE;

/// Dialogs answered from a queue the test fills (`dev.queueOpen`, `dev.queueSave`).
#[derive(Default)]
pub struct ScriptedDialogs {
    open: Mutex<VecDeque<Vec<PathBuf>>>,
    save: Mutex<VecDeque<Option<PathBuf>>>,
}

impl Dialogs for ScriptedDialogs {
    fn open(&self) -> Vec<PathBuf> {
        self.open
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front()
            .unwrap_or_default()
    }
    fn save(&self, _suggested_name: &str) -> Option<PathBuf> {
        self.save
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front()
            .flatten()
    }
    fn folder(&self) -> Option<PathBuf> {
        self.save
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front()
            .flatten()
    }
}

impl ScriptedDialogs {
    pub fn queue_open(&self, paths: Vec<PathBuf>) {
        self.open
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push_back(paths);
    }
    pub fn queue_save(&self, path: Option<PathBuf>) {
        self.save
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push_back(path);
    }
}

/// Fans host events out to every connected client.
#[derive(Default)]
pub struct BridgeSink {
    clients: Mutex<Vec<Sender<Message>>>,
}

impl EventSink for BridgeSink {
    fn emit(&self, event: HostEvent) {
        let text = json!({ "event": event }).to_string();
        let mut g = self.clients.lock().unwrap_or_else(|p| p.into_inner());
        g.retain(|c| c.send(Message::text(text.clone())).is_ok());
    }
}

pub struct Bridge {
    pub addr: SocketAddr,
    pub token: String,
}

impl Bridge {
    pub fn url(&self) -> String {
        format!("ws://{}/{}", self.addr, self.token)
    }
}

fn token() -> String {
    crate::util::random_hex(16)
}

/// Start listening on 127.0.0.1:`port` (0 = any). Returns immediately; threads do the work.
pub fn start(
    broker: Broker,
    sink: Arc<BridgeSink>,
    dialogs: Arc<ScriptedDialogs>,
    port: u16,
) -> std::io::Result<Bridge> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let addr = listener.local_addr()?;
    let tok = token();
    let tok2 = tok.clone();
    std::thread::Builder::new()
        .name("papyrine-bridge".into())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let (b, s, d, t) = (broker.clone(), sink.clone(), dialogs.clone(), tok2.clone());
                let _ = std::thread::Builder::new()
                    .name("papyrine-bridge-client".into())
                    .spawn(move || {
                        let _ = serve_client(stream, b, s, d, &t);
                    });
            }
        })?;
    Ok(Bridge { addr, token: tok })
}

fn origin_ok(origin: Option<&str>) -> bool {
    match origin {
        None => true, // non-browser clients (tests) send no Origin
        Some(o) => {
            let rest = o
                .strip_prefix("http://")
                .or_else(|| o.strip_prefix("https://"));
            rest.is_some_and(|r| r.starts_with("localhost") || r.starts_with("127.0.0.1"))
        }
    }
}

fn serve_client(
    stream: TcpStream,
    broker: Broker,
    sink: Arc<BridgeSink>,
    dialogs: Arc<ScriptedDialogs>,
    tok: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let want = format!("/{tok}");
    #[allow(clippy::result_large_err)] // the handshake callback's signature is fixed
    let check = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
        if req.uri().path() != want || !origin_ok(origin) {
            let mut r = ErrorResponse::new(Some("forbidden".into()));
            *r.status_mut() = StatusCode::FORBIDDEN;
            return Err(r);
        }
        Ok(resp)
    };
    let mut ws: WebSocket<TcpStream> =
        tungstenite::accept_hdr(stream, check).map_err(|e| e.to_string())?;
    ws.get_ref()
        .set_read_timeout(Some(Duration::from_millis(10)))?;
    let (tx, rx): (Sender<Message>, Receiver<Message>) = mpsc::channel();
    sink.clients
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .push(tx.clone());
    loop {
        // Outgoing first: events and finished requests.
        while let Ok(m) = rx.try_recv() {
            ws.send(m)?;
        }
        match ws.read() {
            Ok(Message::Text(t)) => {
                let (b, d, tx) = (broker.clone(), dialogs.clone(), tx.clone());
                std::thread::spawn(move || handle_text(&b, &d, &tx, &t));
            }
            Ok(Message::Binary(bin)) => {
                let (b, tx) = (broker.clone(), tx.clone());
                std::thread::spawn(move || handle_open_bytes(&b, &tx, &bin));
            }
            Ok(Message::Ping(p)) => ws.send(Message::Pong(p))?,
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(());
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn reply(tx: &Sender<Message>, id: u64, r: Result<Value, HostErr>) {
    let v = match r {
        Ok(result) => json!({"id": id, "ok": true, "result": result}),
        Err(e) => json!({"id": id, "ok": false, "error": e}),
    };
    let _ = tx.send(Message::text(v.to_string()));
}

fn tile_frame(id: u64, t: &crate::cache::TileData) -> Message {
    let mut v = Vec::with_capacity(12 + t.rgba.len());
    v.extend_from_slice(&(id as u32).to_le_bytes());
    v.extend_from_slice(&t.width.to_le_bytes());
    v.extend_from_slice(&t.height.to_le_bytes());
    v.extend_from_slice(&t.rgba);
    Message::binary(v)
}

fn handle_open_bytes(b: &Broker, tx: &Sender<Message>, bin: &[u8]) {
    let parse = || -> Option<(u64, String, &[u8])> {
        let id = u32::from_le_bytes(bin.get(0..4)?.try_into().ok()?) as u64;
        let n = u32::from_le_bytes(bin.get(4..8)?.try_into().ok()?) as usize;
        let name = std::str::from_utf8(bin.get(8..8 + n)?).ok()?.to_string();
        Some((id, name, bin.get(8 + n..)?))
    };
    let Some((id, name, data)) = parse() else {
        return;
    };
    reply(
        tx,
        id,
        b.open_bytes(&name, data, None)
            .and_then(|i| serde_json::to_value(i).map_err(Into::into)),
    );
}

fn handle_text(b: &Broker, d: &ScriptedDialogs, tx: &Sender<Message>, text: &str) {
    let Ok(req) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let id = req.get("id").and_then(Value::as_u64).unwrap_or(0);
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let args: Vec<Value> = req
        .get("args")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    match method {
        "getTile" | "getPreview" => {
            let a = args.first().cloned().unwrap_or(Value::Null);
            let doc = a
                .get("docId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let page = a.get("page").and_then(Value::as_u64).unwrap_or(0) as u32;
            let tx2 = tx.clone();
            let cb: crate::sched::Callback = Box::new(move |r| match r {
                Ok(t) => {
                    let _ = tx2.send(tile_frame(id, &t));
                }
                Err(e) => reply(&tx2, id, Err(e)),
            });
            if method == "getTile" {
                let scale = a.get("scale").and_then(Value::as_f64).unwrap_or(1.0);
                let (x, y) = (
                    a.get("tx").and_then(Value::as_u64).unwrap_or(0) as u32,
                    a.get("ty").and_then(Value::as_u64).unwrap_or(0) as u32,
                );
                b.request_tile(&doc, page, scale, x, y, PRIO_VISIBLE, id, cb);
            } else {
                let edge = a.get("edge").and_then(Value::as_u64).unwrap_or(256) as u32;
                b.request_preview(&doc, page, edge, id, cb);
            }
        }
        "cancelTile" => {
            if let Some(rid) = args.first().and_then(Value::as_u64) {
                b.cancel_tile(rid);
            }
            reply(tx, id, Ok(Value::Null));
        }
        m if m.starts_with("dev.") => reply(tx, id, dev(b, d, m, &args)),
        _ => reply(tx, id, dispatch::call(b, method, &args)),
    }
}

fn dev(b: &Broker, d: &ScriptedDialogs, method: &str, args: &[Value]) -> Result<Value, HostErr> {
    match method {
        "dev.killEngine" => {
            b.kill_engine();
            Ok(Value::Null)
        }
        "dev.killRenderer" => {
            b.kill_renderer();
            Ok(Value::Null)
        }
        "dev.queueOpen" => {
            let paths: Vec<String> =
                serde_json::from_value(args.first().cloned().unwrap_or(json!([])))?;
            d.queue_open(paths.into_iter().map(PathBuf::from).collect());
            Ok(Value::Null)
        }
        "dev.queueSave" => {
            let p: Option<String> =
                serde_json::from_value(args.first().cloned().unwrap_or(Value::Null))?;
            d.queue_save(p.map(PathBuf::from));
            Ok(Value::Null)
        }
        "dev.stats" => Ok(b.stats()),
        "dev.scanRecovery" => {
            b.scan_recovery();
            Ok(Value::Null)
        }
        "dev.shutdown" => {
            b.shutdown();
            Ok(Value::Null)
        }
        other => Err(HostErr::internal(format!("unknown dev method {other:?}"))),
    }
}
