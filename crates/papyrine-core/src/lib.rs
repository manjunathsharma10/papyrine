//! Shared primitives for every Papyrine crate: errors, geometry, units,
//! cancellation, progress reporting, id newtypes and the startup trace.
//!
//! This crate has no dependencies and knows nothing about qpdf or PDF objects.

pub mod cancel;
pub mod error;
pub mod geom;
pub mod ids;
pub mod progress;
pub mod startup;
pub mod units;

pub use cancel::{CancelPoller, CancelToken, Cancelled};
pub use error::{Error, ErrorKind, Result};
pub use geom::{Matrix, Point, Rect, Size};
pub use ids::{IdGen, JobId, RequestId};
pub use progress::{FnProgress, NullProgress, Progress, SubProgress};
pub use units::{PageSize, Unit};
