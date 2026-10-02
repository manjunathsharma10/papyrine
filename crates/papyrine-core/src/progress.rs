//! Progress reporting for long operations.

/// Implementations must be cheap and must not block; callers may report from
/// worker threads.
pub trait Progress: Send + Sync {
    /// `total` is `None` when unknown. `done <= total` when both are known.
    fn report(&self, done: u64, total: Option<u64>, message: Option<&str>);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NullProgress;

impl Progress for NullProgress {
    fn report(&self, _: u64, _: Option<u64>, _: Option<&str>) {}
}

/// Adapts a closure `(done, total, message)`.
pub struct FnProgress<F>(pub F);

impl<F> Progress for FnProgress<F>
where
    F: Fn(u64, Option<u64>, Option<&str>) + Send + Sync,
{
    fn report(&self, done: u64, total: Option<u64>, message: Option<&str>) {
        (self.0)(done, total, message)
    }
}

/// Maps a child task's 0..=total progress into a slice of a parent's range, so
/// multi-phase jobs report one monotonic bar.
pub struct SubProgress<'a> {
    parent: &'a dyn Progress,
    base: u64,
    span: u64,
    parent_total: u64,
}

impl<'a> SubProgress<'a> {
    pub fn new(parent: &'a dyn Progress, base: u64, span: u64, parent_total: u64) -> Self {
        Self {
            parent,
            base,
            span,
            parent_total,
        }
    }
}

impl Progress for SubProgress<'_> {
    fn report(&self, done: u64, total: Option<u64>, message: Option<&str>) {
        let mapped = match total {
            Some(t) if t > 0 => {
                let d = done.min(t);
                ((u128::from(d) * u128::from(self.span)) / u128::from(t)) as u64
            }
            _ => 0,
        };
        self.parent
            .report(self.base + mapped, Some(self.parent_total), message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn sub_progress_maps_range() {
        let seen = Mutex::new(Vec::new());
        let parent = FnProgress(|d, t, _: Option<&str>| seen.lock().unwrap().push((d, t)));
        let sub = SubProgress::new(&parent, 50, 25, 100);
        sub.report(0, Some(10), None);
        sub.report(5, Some(10), None);
        sub.report(10, Some(10), None);
        sub.report(99, Some(10), None); // clamped
        sub.report(3, None, None);
        let v = seen.lock().unwrap().clone();
        assert_eq!(
            v,
            vec![
                (50, Some(100)),
                (62, Some(100)),
                (75, Some(100)),
                (75, Some(100)),
                (50, Some(100))
            ]
        );
    }

    #[test]
    fn null_progress_is_noop() {
        NullProgress.report(1, None, Some("x"));
    }
}
