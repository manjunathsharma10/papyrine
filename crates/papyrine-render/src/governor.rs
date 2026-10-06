//! Memory policy of the renderer (ARCHITECTURE §1.2, ADR-009).
//!
//! PDFium keeps three things that grow while a document is scrolled:
//! the parsed pages in its page cache (decoded images included, up to 25 MB
//! per image-heavy page), the document's object store (every stream and font
//! it has parsed, released only by closing the document), and allocator
//! caches. The governor decides, from the process footprint relative to what
//! the open documents cost by themselves, when to close pages, when to
//! recycle (close and re-open: under 1 ms through [`crate::MultiBuf`] for
//! ordinary files) and when to hibernate a document whose parse state alone
//! is expensive. It is pure so the thresholds can be tested without PDFium.

use std::time::Duration;

const MIB: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct MemoryConfig {
    /// Growth above the documents' own cost at which every page except the
    /// active one is closed.
    pub page_trim: u64,
    /// Growth at which the document is re-opened before the next cold page load.
    pub recycle: u64,
    /// Re-open anyway after this many page parses (bounds the object store
    /// where the footprint cannot be measured).
    pub max_page_loads: u64,
    /// No job for this long: close all pages.
    pub idle_after: Duration,
    /// At idle, growth above this triggers a recycle.
    pub idle_recycle: u64,
    /// No job for this long: documents whose open cost is at least
    /// [`Self::hibernate_cost`] are dropped (re-opened by the next request).
    pub hibernate_after: Duration,
    pub hibernate_cost: u64,
    /// Budget of the per-page text cache.
    pub text_cache_bytes: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        MemoryConfig {
            page_trim: 32 * MIB,
            recycle: 64 * MIB,
            max_page_loads: 256,
            idle_after: Duration::from_millis(1500),
            idle_recycle: 8 * MIB,
            hibernate_after: Duration::from_millis(3000),
            hibernate_cost: 24 * MIB,
            text_cache_bytes: 3 * 1024 * 1024,
        }
    }
}

/// What to do before loading a page that is not cached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColdLoad {
    Proceed,
    /// Close the other cached pages first.
    TrimPages,
    /// Re-open the document first.
    Recycle,
}

#[derive(Clone, Debug)]
pub struct Governor {
    pub cfg: MemoryConfig,
    /// A recycle that did not help backs off until this many cold loads have passed.
    backoff: u32,
}

impl Governor {
    pub fn new(cfg: MemoryConfig) -> Self {
        Governor { cfg, backoff: 0 }
    }

    pub fn before_cold_load(&mut self, growth: u64, loads_since_open: u64) -> ColdLoad {
        if self.backoff > 0 {
            self.backoff -= 1;
            return if growth > self.cfg.page_trim {
                ColdLoad::TrimPages
            } else {
                ColdLoad::Proceed
            };
        }
        if growth > self.cfg.recycle || loads_since_open >= self.cfg.max_page_loads {
            ColdLoad::Recycle
        } else if growth > self.cfg.page_trim {
            ColdLoad::TrimPages
        } else {
            ColdLoad::Proceed
        }
    }

    /// After a job: shrink to the active page when growth is high.
    pub fn trim_after_job(&self, growth: u64) -> bool {
        growth > self.cfg.page_trim
    }

    /// Report the result of a recycle. When it freed less than a quarter of
    /// the growth (a platform that keeps freed memory), stop recycling for a while.
    pub fn recycled(&mut self, before: u64, after: u64) {
        if before > 4 * MIB && after * 4 > before * 3 {
            self.backoff = 64;
        }
    }

    pub fn idle_should_recycle(&self, growth: u64) -> bool {
        growth > self.cfg.idle_recycle
    }

    pub fn should_hibernate(&self, open_cost: u64) -> bool {
        open_cost >= self.cfg.hibernate_cost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds() {
        let mut g = Governor::new(MemoryConfig::default());
        assert_eq!(g.before_cold_load(MIB, 3), ColdLoad::Proceed);
        assert_eq!(g.before_cold_load(40 * MIB, 3), ColdLoad::TrimPages);
        assert_eq!(g.before_cold_load(70 * MIB, 3), ColdLoad::Recycle);
        assert_eq!(g.before_cold_load(MIB, 256), ColdLoad::Recycle);
    }

    #[test]
    fn useless_recycles_back_off() {
        let mut g = Governor::new(MemoryConfig::default());
        g.recycled(100 * MIB, 95 * MIB);
        assert_eq!(g.before_cold_load(100 * MIB, 3), ColdLoad::TrimPages);
        g.recycled(100 * MIB, 10 * MIB); // a good recycle does not extend it
        for _ in 0..70 {
            g.before_cold_load(1, 0);
        }
        assert_eq!(g.before_cold_load(70 * MIB, 3), ColdLoad::Recycle);
    }

    #[test]
    fn idle_and_hibernate() {
        let g = Governor::new(MemoryConfig::default());
        assert!(!g.idle_should_recycle(MIB));
        assert!(g.idle_should_recycle(9 * MIB));
        assert!(!g.should_hibernate(10 * MIB));
        assert!(g.should_hibernate(100 * MIB));
    }
}
