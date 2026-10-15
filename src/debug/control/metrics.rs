//! Fixed in-process counters. No payload, identifier, histogram or label storage.
use std::{
    sync::{
        Mutex, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

fn add(counter: &AtomicU64, amount: u64) {
    let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
        Some(value.saturating_add(amount))
    });
}

#[derive(Default)]
pub(crate) struct DurationMetric {
    samples: AtomicU64,
    total_ns: AtomicU64,
    max_ns: AtomicU64,
}
impl DurationMetric {
    pub(crate) fn record(&self, elapsed: std::time::Duration) {
        let elapsed = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
        add(&self.samples, 1);
        add(&self.total_ns, elapsed);
        self.max_ns.fetch_max(elapsed, Ordering::Relaxed);
    }
    pub(crate) fn start(&self) -> Timing<'_> {
        Timing {
            metric: self,
            started: Instant::now(),
        }
    }
    #[cfg(test)]
    fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({"samples":self.samples.load(Ordering::Relaxed), "total_ns":self.total_ns.load(Ordering::Relaxed), "max_ns":self.max_ns.load(Ordering::Relaxed)})
    }
}
pub(crate) struct Timing<'a> {
    metric: &'a DurationMetric,
    started: Instant,
}
impl Drop for Timing<'_> {
    fn drop(&mut self) {
        self.metric.record(self.started.elapsed());
    }
}

#[derive(Default)]
pub(crate) struct Metrics {
    accepted: AtomicU64,
    completed: AtomicU64,
    failed: AtomicU64,
    rejected: AtomicU64,
    capture_peak: AtomicU64,
    capture_capacity: Mutex<Weak<tokio::sync::Semaphore>>,
    pub(crate) queue: DurationMetric,
    pub(crate) encoding: DurationMetric,
}
impl Metrics {
    pub(crate) fn accepted(&self) {
        add(&self.accepted, 1);
    }
    pub(crate) fn completed(&self) {
        add(&self.completed, 1);
    }
    pub(crate) fn failed(&self) {
        add(&self.failed, 1);
    }
    pub(crate) fn rejected(&self) {
        add(&self.rejected, 1);
    }
    pub(crate) fn capture_started(&self, capacity: &std::sync::Arc<tokio::sync::Semaphore>) {
        *self
            .capture_capacity
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = std::sync::Arc::downgrade(capacity);
        self.capture_peak.fetch_max(1, Ordering::Relaxed);
    }
    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> serde_json::Value {
        let inflight = self
            .capture_capacity
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .upgrade()
            .map_or(0, |capacity| {
                1usize.saturating_sub(capacity.available_permits())
            });
        serde_json::json!({
            "input":{"accepted":self.accepted.load(Ordering::Relaxed),"completed":self.completed.load(Ordering::Relaxed),"failed":self.failed.load(Ordering::Relaxed)},
            "rejected_requests":self.rejected.load(Ordering::Relaxed),
            "capture":{"inflight":inflight,"peak":self.capture_peak.load(Ordering::Relaxed)},
            "queue":self.queue.snapshot(),"encoding":self.encoding.snapshot()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisoned_snapshot_lock_cannot_break_capture_accounting() {
        let metrics = Metrics::default();
        let poisoned = std::panic::catch_unwind(|| {
            let _guard = metrics.capture_capacity.lock().unwrap();
            panic!("test-only interruption while holding the snapshot lock");
        });
        assert!(poisoned.is_err());
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        metrics.capture_started(&capacity);
        assert_eq!(metrics.snapshot()["capture"]["inflight"], 1);
        drop(permit);
        assert_eq!(metrics.snapshot()["capture"]["inflight"], 0);
    }

    #[test]
    fn counters_and_duration_totals_saturate_without_wrapping() {
        let counter = AtomicU64::new(u64::MAX - 1);
        add(&counter, 2);
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
        let duration = DurationMetric::default();
        duration.record(std::time::Duration::MAX);
        duration.record(std::time::Duration::from_nanos(1));
        assert_eq!(duration.samples.load(Ordering::Relaxed), 2);
        assert_eq!(duration.total_ns.load(Ordering::Relaxed), u64::MAX);
        assert_eq!(duration.max_ns.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn fixed_storage_and_weak_capacity_do_not_own_pipeline_lifetime() {
        let metrics = Metrics::default();
        assert!(std::mem::size_of::<Metrics>() <= 256);
        assert_eq!(metrics.snapshot()["capture"]["inflight"], 0);
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        metrics.capture_started(&capacity);
        assert_eq!(metrics.snapshot()["capture"]["inflight"], 1);
        drop(permit);
        assert_eq!(metrics.snapshot()["capture"]["inflight"], 0);
        drop(capacity);
        assert_eq!(metrics.snapshot()["capture"]["inflight"], 0);
        println!(
            "Fixed metrics storage: {} bytes",
            std::mem::size_of::<Metrics>()
        );
    }
}
