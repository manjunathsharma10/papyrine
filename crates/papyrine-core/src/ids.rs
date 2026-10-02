//! Typed ids so request and job numbers cannot be mixed up.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

macro_rules! id_newtype {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u64);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($prefix, "{}"), self.0)
            }
        }

        impl From<u64> for $name {
            fn from(v: u64) -> Self {
                Self(v)
            }
        }
    };
}

id_newtype!(RequestId, "req-");
id_newtype!(JobId, "job-");

/// Monotonic id source, starting at 1 so 0 can mean "none" on the wire.
#[derive(Debug, Default)]
pub struct IdGen(AtomicU64);

impl IdGen {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn next_u64(&self) -> u64 {
        self.0.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn next_request(&self) -> RequestId {
        RequestId(self.next_u64())
    }

    pub fn next_job(&self) -> JobId {
        JobId(self.next_u64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_monotonic_and_display() {
        let g = IdGen::new();
        assert_eq!(g.next_request(), RequestId(1));
        assert_eq!(g.next_job(), JobId(2));
        assert_eq!(JobId(7).to_string(), "job-7");
        assert_eq!(RequestId(3).to_string(), "req-3");
    }
}
