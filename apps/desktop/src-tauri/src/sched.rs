//! Tile scheduler: a priority queue with de-duplication and cancellation in front of
//! the single renderer process (ARCHITECTURE §4.6).
//!
//! * Visible tiles outrank previews, which outrank prefetch. Within a priority the
//!   newest request goes first, because the user has scrolled on since the old one.
//! * Identical requests share one render.
//! * A request nobody waits for any more is dropped before it starts; one that is
//!   already rendering gets its cancel flag set so the renderer can stop early.
//! * The worker count is small (2): the renderer is serial, the second request just
//!   hides the IPC round trip.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use crate::cache::{TileData, TileKey};
use crate::error::{HostErr, Result};

pub const PRIO_VISIBLE: u8 = 0;
pub const PRIO_PREVIEW: u8 = 1;
pub const PRIO_PREFETCH: u8 = 2;

pub type Callback = Box<dyn FnOnce(Result<Arc<TileData>>) + Send>;

/// Renders one tile; polls `cancel` and returns `Cancelled` when it is set.
pub type RenderFn = dyn Fn(&TileKey, &AtomicBool) -> Result<Arc<TileData>> + Send + Sync;

struct Waiter {
    rid: u64,
    cb: Callback,
}

struct Pending {
    key: TileKey,
    prio: u8,
    seq: u64,
    waiters: Vec<Waiter>,
}

struct Running {
    cancel: Arc<AtomicBool>,
    waiters: Vec<Waiter>,
}

#[derive(Default)]
struct State {
    queue: Vec<Pending>,
    running: HashMap<TileKey, Running>,
    seq: u64,
    shutdown: bool,
    rendered: u64,
    dropped: u64,
}

struct Shared {
    st: Mutex<State>,
    cv: Condvar,
}

pub struct TileSched {
    shared: Arc<Shared>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl TileSched {
    pub fn new(workers: usize, render: Arc<RenderFn>) -> Self {
        let shared = Arc::new(Shared {
            st: Mutex::new(State::default()),
            cv: Condvar::new(),
        });
        let handles = (0..workers.max(1))
            .map(|i| {
                let sh = shared.clone();
                let r = render.clone();
                std::thread::Builder::new()
                    .name(format!("papyrine-tiles-{i}"))
                    .spawn(move || worker(sh, r))
                    .expect("spawn tile worker")
            })
            .collect();
        Self {
            shared,
            workers: Mutex::new(handles),
        }
    }

    /// Queue a render. `rid` identifies this waiter for [`cancel`](Self::cancel).
    pub fn submit(&self, key: TileKey, prio: u8, rid: u64, cb: Callback) {
        let mut g = lock(&self.shared.st);
        if g.shutdown {
            drop(g);
            cb(Err(HostErr::cancelled()));
            return;
        }
        if let Some(r) = g.running.get_mut(&key) {
            r.waiters.push(Waiter { rid, cb });
            return;
        }
        if let Some(p) = g.queue.iter_mut().find(|p| p.key == key) {
            p.prio = p.prio.min(prio);
            p.waiters.push(Waiter { rid, cb });
            return;
        }
        g.seq += 1;
        let seq = g.seq;
        g.queue.push(Pending {
            key,
            prio,
            seq,
            waiters: vec![Waiter { rid, cb }],
        });
        drop(g);
        self.shared.cv.notify_one();
    }

    /// The waiter `rid` lost interest. Its callback receives `Cancelled`.
    pub fn cancel(&self, rid: u64) {
        let mut cancelled: Vec<Callback> = Vec::new();
        {
            let mut g = lock(&self.shared.st);
            let mut dropped = 0;
            g.queue.retain_mut(|p| {
                if let Some(i) = p.waiters.iter().position(|w| w.rid == rid) {
                    cancelled.push(p.waiters.remove(i).cb);
                }
                if p.waiters.is_empty() {
                    dropped += 1;
                    false
                } else {
                    true
                }
            });
            g.dropped += dropped;
            for r in g.running.values_mut() {
                if let Some(i) = r.waiters.iter().position(|w| w.rid == rid) {
                    cancelled.push(r.waiters.remove(i).cb);
                    if r.waiters.is_empty() {
                        r.cancel.store(true, Ordering::Relaxed);
                    }
                }
            }
        }
        for cb in cancelled {
            cb(Err(HostErr::cancelled()));
        }
    }

    /// Drop everything queued for a document (closing it); running renders are cancelled.
    pub fn purge_doc(&self, doc: u64) {
        let mut cbs: Vec<Callback> = Vec::new();
        {
            let mut g = lock(&self.shared.st);
            let mut keep = Vec::new();
            for p in g.queue.drain(..) {
                if p.key.doc == doc {
                    cbs.extend(p.waiters.into_iter().map(|w| w.cb));
                } else {
                    keep.push(p);
                }
            }
            g.queue = keep;
            for (k, r) in g.running.iter_mut() {
                if k.doc == doc {
                    r.cancel.store(true, Ordering::Relaxed);
                    cbs.extend(r.waiters.drain(..).map(|w| w.cb));
                }
            }
        }
        for cb in cbs {
            cb(Err(HostErr::cancelled()));
        }
    }

    pub fn queued(&self) -> usize {
        lock(&self.shared.st).queue.len()
    }

    /// (rendered, dropped-before-start) counters.
    pub fn counters(&self) -> (u64, u64) {
        let g = lock(&self.shared.st);
        (g.rendered, g.dropped)
    }

    pub fn shutdown(&self) {
        let mut cbs: Vec<Callback> = Vec::new();
        {
            let mut g = lock(&self.shared.st);
            g.shutdown = true;
            for p in g.queue.drain(..) {
                cbs.extend(p.waiters.into_iter().map(|w| w.cb));
            }
            for r in g.running.values_mut() {
                r.cancel.store(true, Ordering::Relaxed);
            }
        }
        self.shared.cv.notify_all();
        for cb in cbs {
            cb(Err(HostErr::cancelled()));
        }
        let hs: Vec<_> = lock(&self.workers).drain(..).collect();
        for h in hs {
            let _ = h.join();
        }
    }
}

impl Drop for TileSched {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn worker(sh: Arc<Shared>, render: Arc<RenderFn>) {
    loop {
        let (key, cancel) = {
            let mut g = lock(&sh.st);
            loop {
                if g.shutdown {
                    return;
                }
                // Best = lowest prio value, then newest.
                let best = g
                    .queue
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| !g.running.contains_key(&p.key))
                    .min_by_key(|(_, p)| (p.prio, std::cmp::Reverse(p.seq)))
                    .map(|(i, _)| i);
                if let Some(i) = best {
                    let p = g.queue.swap_remove(i);
                    let cancel = Arc::new(AtomicBool::new(false));
                    g.running.insert(
                        p.key,
                        Running {
                            cancel: cancel.clone(),
                            waiters: p.waiters,
                        },
                    );
                    break (p.key, cancel);
                }
                g = sh.cv.wait(g).unwrap_or_else(|p| p.into_inner());
            }
        };
        let result = render(&key, &cancel);
        let waiters = {
            let mut g = lock(&sh.st);
            g.rendered += 1;
            g.running
                .remove(&key)
                .map(|r| r.waiters)
                .unwrap_or_default()
        };
        match &result {
            // Cancelled while someone still wanted it (a late joiner): render again.
            Err(e)
                if e.is_cancelled() && !waiters.is_empty() && !cancel.load(Ordering::Relaxed) =>
            {
                let mut g = lock(&sh.st);
                g.seq += 1;
                let seq = g.seq;
                g.queue.push(Pending {
                    key,
                    prio: PRIO_VISIBLE,
                    seq,
                    waiters,
                });
                drop(g);
                sh.cv.notify_one();
            }
            _ => {
                for w in waiters {
                    (w.cb)(result.clone());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn key(page: u32) -> TileKey {
        TileKey {
            doc: 1,
            page,
            bucket: 0,
            tx: 0,
            ty: 0,
        }
    }

    fn tile() -> Arc<TileData> {
        Arc::new(TileData {
            width: 1,
            height: 1,
            rgba: vec![0; 4],
        })
    }

    #[test]
    fn newest_visible_first_and_dedup() {
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let gate_rx = Mutex::new(gate_rx);
        let order = Arc::new(Mutex::new(Vec::new()));
        let o2 = order.clone();
        let s = TileSched::new(
            1,
            Arc::new(move |k: &TileKey, _c: &AtomicBool| {
                o2.lock().unwrap().push(k.page);
                if k.page == 0 {
                    gate_rx.lock().unwrap().recv().unwrap();
                }
                Ok(tile())
            }),
        );
        let (tx, rx) = mpsc::channel();
        let mk = |tx: &mpsc::Sender<u32>, p: u32| {
            let tx = tx.clone();
            Box::new(move |r: Result<Arc<TileData>>| {
                assert!(r.is_ok());
                tx.send(p).unwrap();
            }) as Callback
        };
        s.submit(key(0), PRIO_VISIBLE, 1, mk(&tx, 0));
        std::thread::sleep(Duration::from_millis(50)); // page 0 is rendering
        s.submit(key(1), PRIO_VISIBLE, 2, mk(&tx, 1));
        s.submit(key(2), PRIO_PREFETCH, 3, mk(&tx, 2));
        s.submit(key(3), PRIO_VISIBLE, 4, mk(&tx, 3));
        s.submit(key(3), PRIO_VISIBLE, 5, mk(&tx, 33)); // same tile: one render, two callbacks
        gate_tx.send(()).unwrap();
        let mut got = Vec::new();
        for _ in 0..5 {
            got.push(rx.recv_timeout(Duration::from_secs(2)).unwrap());
        }
        assert_eq!(*order.lock().unwrap(), vec![0, 3, 1, 2]);
        got.sort();
        assert_eq!(got, vec![0, 1, 2, 3, 33]);
    }

    #[test]
    fn cancel_before_start_never_renders() {
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let gate_rx = Mutex::new(gate_rx);
        let rendered = Arc::new(Mutex::new(Vec::new()));
        let r2 = rendered.clone();
        let s = TileSched::new(
            1,
            Arc::new(move |k: &TileKey, _c: &AtomicBool| {
                r2.lock().unwrap().push(k.page);
                if k.page == 0 {
                    gate_rx.lock().unwrap().recv().unwrap();
                }
                Ok(tile())
            }),
        );
        let (tx, rx) = mpsc::channel();
        let tx0 = tx.clone();
        s.submit(
            key(0),
            0,
            1,
            Box::new(move |r| tx0.send(r.is_ok()).unwrap()),
        );
        std::thread::sleep(Duration::from_millis(50));
        let tx1 = tx.clone();
        s.submit(
            key(1),
            0,
            2,
            Box::new(move |r| tx1.send(r.is_ok()).unwrap()),
        );
        s.cancel(2);
        assert!(!rx.recv_timeout(Duration::from_secs(1)).unwrap());
        gate_tx.send(()).unwrap();
        assert!(rx.recv_timeout(Duration::from_secs(1)).unwrap());
        assert_eq!(*rendered.lock().unwrap(), vec![0]);
        assert_eq!(s.counters().1, 1);
    }

    #[test]
    fn cancel_while_running_sets_the_flag() {
        let started = Arc::new(AtomicBool::new(false));
        let st = started.clone();
        let s = TileSched::new(
            1,
            Arc::new(move |_k: &TileKey, c: &AtomicBool| {
                st.store(true, Ordering::SeqCst);
                for _ in 0..400 {
                    if c.load(Ordering::Relaxed) {
                        return Err(HostErr::cancelled());
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(tile())
            }),
        );
        let (tx, rx) = mpsc::channel();
        s.submit(key(0), 0, 9, Box::new(move |r| tx.send(r).unwrap()));
        while !started.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
        let t0 = std::time::Instant::now();
        s.cancel(9);
        assert!(
            rx.recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap_err()
                .is_cancelled()
        );
        assert!(t0.elapsed() < Duration::from_millis(500));
    }
}
