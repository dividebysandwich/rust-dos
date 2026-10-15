//! Test-visible snapshot of the shared service's actual fixed counters.
pub(crate) fn snapshot(shared: &crate::debug::Shared) -> Option<serde_json::Value> {
    Some(shared.metrics.snapshot())
}
