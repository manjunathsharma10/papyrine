//! Cooperative cancellation. Tokens are cheap to clone and share one flag.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Interval at which long loops should look at the flag.
pub const CHECK_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("operation cancelled")
    }
}

impl std::error::Error for Cancelled {}

#[derive(Clone, Debug, Default)]
pub struct CancelToken {
    flag: Arc<AtomicBool>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }

    /// `Err(Cancelled)` once cancelled; use with `?` in hot loops that already
    /// run at a coarse granularity.
    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() {
            Err(Cancelled)
        } else {
            Ok(())
        }
    }

    /// A poller for tight loops: `tick` is nearly free and only reads the flag
    /// about every [`CHECK_INTERVAL`].
    pub fn poller(&self) -> CancelPoller {
        CancelPoller::with_interval(self.clone(), CHECK_INTERVAL)
    }
}

#[derive(Debug)]
pub struct CancelPoller {
    token: CancelToken,
    interval: Duration,
    last: Instant,
    calls: u32,
}

impl CancelPoller {
    pub fn with_interval(token: CancelToken, interval: Duration) -> Self {
        Self {
            token,
            interval,
            last: Instant::now(),
            calls: 0,
        }
    }

    /// Call once per loop iteration. Reads the clock only every 64 calls and
    /// the flag only when the interval has elapsed.
    #[inline]
    pub fn tick(&mut self) -> Result<(), Cancelled> {
        self.calls = self.calls.wrapping_add(1);
        if self.calls & 63 != 0 {
            return Ok(());
        }
        let now = Instant::now();
        if now.duration_since(self.last) >= self.interval {
            self.last = now;
            return self.token.check();
        }
        Ok(())
    }

    /// Unconditional check, for boundaries between phases.
    pub fn check_now(&mut self) -> Result<(), Cancelled> {
        self.last = Instant::now();
        self.token.check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clones_share_flag() {
        let a = CancelToken::new();
        let b = a.clone();
        assert!(b.check().is_ok());
        a.cancel();
        assert_eq!(b.check(), Err(Cancelled));
    }

    #[test]
    fn poller_notices_cancellation_within_interval() {
        let t = CancelToken::new();
        let mut p = CancelPoller::with_interval(t.clone(), Duration::from_millis(1));
        for _ in 0..1000 {
            assert!(p.tick().is_ok());
        }
        t.cancel();
        std::thread::sleep(Duration::from_millis(3));
        let mut seen = false;
        for _ in 0..200 {
            if p.tick().is_err() {
                seen = true;
                break;
            }
        }
        assert!(seen);
    }

    #[test]
    fn check_now_is_immediate() {
        let t = CancelToken::new();
        let mut p = t.poller();
        t.cancel();
        assert!(p.check_now().is_err());
    }
}
