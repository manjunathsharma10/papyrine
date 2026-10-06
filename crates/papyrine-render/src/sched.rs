//! Priority queue with cancellation for the one PDFium thread (ARCHITECTURE §4.6).
//!
//! Classes, highest first: control (open, close, re-open, sizes), visible
//! tiles, previews and page text, prefetch tiles, thumbnails, search slices.
//! Within visible/preview/prefetch the **newest** request runs first (the page
//! the user just scrolled to), the other classes are first in, first out.

use papyrine_core::CancelToken;
use papyrine_ipc::{JobId, RequestId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Priority {
    Search = 1,
    Thumbnail = 2,
    Prefetch = 3,
    Preview = 4,
    Visible = 5,
    Control = 6,
}

impl Priority {
    fn from_u8(v: u8) -> Option<Priority> {
        Some(match v {
            1 => Priority::Search,
            2 => Priority::Thumbnail,
            3 => Priority::Prefetch,
            4 => Priority::Preview,
            5 => Priority::Visible,
            6 => Priority::Control,
            _ => return None,
        })
    }

    fn newest_first(self) -> bool {
        matches!(
            self,
            Priority::Visible | Priority::Preview | Priority::Prefetch
        )
    }
}

pub struct Item<T> {
    pub seq: u64,
    pub prio: Priority,
    pub id: RequestId,
    pub job: Option<JobId>,
    pub cancel: CancelToken,
    pub work: T,
}

pub enum Popped<T> {
    Item(Item<T>),
    Timeout,
    /// The queue was closed; the worker should stop.
    Closed,
}

struct State<T> {
    items: Vec<Item<T>>,
    jobs: HashMap<JobId, CancelToken>,
    seq: u64,
    closed: bool,
}

pub struct Queue<T> {
    st: Mutex<State<T>>,
    cv: Condvar,
    /// Highest queued priority (0 = empty), readable without the lock from
    /// inside PDFium's pause callback.
    top: AtomicU8,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Queue<T> {
    pub fn new() -> Self {
        Queue {
            st: Mutex::new(State {
                items: Vec::new(),
                jobs: HashMap::new(),
                seq: 0,
                closed: false,
            }),
            cv: Condvar::new(),
            top: AtomicU8::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State<T>> {
        self.st.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn refresh_top(&self, st: &State<T>) {
        let t = st.items.iter().map(|i| i.prio as u8).max().unwrap_or(0);
        self.top.store(t, Ordering::Release);
    }

    /// Highest priority currently waiting.
    pub fn top(&self) -> Option<Priority> {
        Priority::from_u8(self.top.load(Ordering::Acquire))
    }

    /// True when something strictly above `p` is waiting (lock-free).
    pub fn higher_than(&self, p: Priority) -> bool {
        self.top.load(Ordering::Acquire) > p as u8
    }

    pub fn len(&self) -> usize {
        self.lock().items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Queue new work. A request with a job id is registered for [`Self::cancel`].
    pub fn push(&self, prio: Priority, id: RequestId, job: Option<JobId>, work: T) -> CancelToken {
        let mut st = self.lock();
        let cancel = match job.and_then(|j| st.jobs.get(&j).cloned()) {
            Some(t) => t, // a re-queued continuation keeps its token
            None => {
                let t = CancelToken::new();
                if let Some(j) = job {
                    st.jobs.insert(j, t.clone());
                }
                t
            }
        };
        st.seq += 1;
        let seq = st.seq;
        st.items.push(Item {
            seq,
            prio,
            id,
            job,
            cancel: cancel.clone(),
            work,
        });
        self.refresh_top(&st);
        drop(st);
        self.cv.notify_one();
        cancel
    }

    /// Forget a finished job's token.
    pub fn finish_job(&self, job: JobId) {
        self.lock().jobs.remove(&job);
    }

    /// Cancel `job`. A request still waiting in the queue is removed and
    /// returned so the caller can answer it; a running one sees its token.
    pub fn cancel(&self, job: JobId) -> Option<Item<T>> {
        let mut st = self.lock();
        if let Some(t) = st.jobs.get(&job) {
            t.cancel();
        }
        let pos = st.items.iter().position(|i| i.job == Some(job))?;
        let item = st.items.remove(pos);
        st.jobs.remove(&job);
        self.refresh_top(&st);
        Some(item)
    }

    pub fn close(&self) {
        self.lock().closed = true;
        self.cv.notify_all();
    }

    pub fn wake(&self) {
        self.cv.notify_all();
    }

    fn take_best(st: &mut State<T>) -> Option<Item<T>> {
        let mut best: Option<usize> = None;
        for (i, it) in st.items.iter().enumerate() {
            best = Some(match best {
                None => i,
                Some(b) => {
                    let cur = &st.items[b];
                    let better = it.prio > cur.prio
                        || (it.prio == cur.prio
                            && if it.prio.newest_first() {
                                it.seq > cur.seq
                            } else {
                                it.seq < cur.seq
                            });
                    if better { i } else { b }
                }
            });
        }
        best.map(|i| st.items.swap_remove(i))
    }

    /// Next item by priority; waits up to `timeout` (forever for `None`).
    pub fn pop(&self, timeout: Option<Duration>) -> Popped<T> {
        let deadline = timeout.map(|d| std::time::Instant::now() + d);
        let mut st = self.lock();
        loop {
            if st.closed {
                return Popped::Closed;
            }
            if let Some(it) = Self::take_best(&mut st) {
                self.refresh_top(&st);
                return Popped::Item(it);
            }
            st = match deadline {
                None => self.cv.wait(st).unwrap_or_else(|p| p.into_inner()),
                Some(d) => {
                    let now = std::time::Instant::now();
                    if now >= d {
                        return Popped::Timeout;
                    }
                    self.cv
                        .wait_timeout(st, d - now)
                        .unwrap_or_else(|p| p.into_inner())
                        .0
                }
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rid(n: u64) -> RequestId {
        RequestId(n)
    }

    fn pop_all(q: &Queue<u32>) -> Vec<u32> {
        let mut v = vec![];
        while let Popped::Item(i) = q.pop(Some(Duration::ZERO)) {
            v.push(i.work);
        }
        v
    }

    #[test]
    fn classes_and_order() {
        let q = Queue::new();
        q.push(Priority::Search, rid(1), None, 1);
        q.push(Priority::Thumbnail, rid(2), None, 2);
        q.push(Priority::Thumbnail, rid(3), None, 3);
        q.push(Priority::Prefetch, rid(4), None, 4);
        q.push(Priority::Visible, rid(5), None, 5);
        q.push(Priority::Visible, rid(6), None, 6);
        q.push(Priority::Preview, rid(7), None, 7);
        q.push(Priority::Control, rid(8), None, 8);
        q.push(Priority::Control, rid(9), None, 9);
        // control FIFO, visible newest first, preview, prefetch, thumbnails FIFO, search
        assert_eq!(pop_all(&q), vec![8, 9, 6, 5, 7, 4, 2, 3, 1]);
        assert!(q.top().is_none());
    }

    #[test]
    fn higher_than_tracks_the_top() {
        let q = Queue::new();
        assert!(!q.higher_than(Priority::Search));
        q.push(Priority::Thumbnail, rid(1), None, 1);
        assert!(q.higher_than(Priority::Search));
        assert!(!q.higher_than(Priority::Thumbnail));
        q.push(Priority::Visible, rid(2), None, 2);
        assert!(q.higher_than(Priority::Preview));
        pop_all(&q);
        assert!(!q.higher_than(Priority::Search));
    }

    #[test]
    fn cancel_removes_queued_and_flags_running() {
        let q = Queue::new();
        let t = q.push(Priority::Visible, rid(1), Some(JobId(7)), 1);
        q.push(Priority::Visible, rid(2), Some(JobId(8)), 2);
        let removed = q.cancel(JobId(7)).expect("queued");
        assert_eq!(removed.work, 1);
        assert!(t.is_cancelled());
        assert_eq!(q.len(), 1);
        // A running job (already popped) is only flagged.
        let Popped::Item(running) = q.pop(Some(Duration::ZERO)) else {
            panic!()
        };
        assert!(q.cancel(JobId(8)).is_none());
        assert!(running.cancel.is_cancelled());
    }

    #[test]
    fn requeued_job_keeps_its_token() {
        let q = Queue::new();
        let t = q.push(Priority::Search, rid(1), Some(JobId(1)), 1);
        let Popped::Item(it) = q.pop(Some(Duration::ZERO)) else {
            panic!()
        };
        let t2 = q.push(Priority::Search, it.id, it.job, 2);
        t.cancel();
        assert!(t2.is_cancelled());
        q.finish_job(JobId(1));
    }

    #[test]
    fn timeout_and_close() {
        let q: Queue<u32> = Queue::new();
        assert!(matches!(
            q.pop(Some(Duration::from_millis(5))),
            Popped::Timeout
        ));
        q.close();
        assert!(matches!(q.pop(None), Popped::Closed));
    }
}
