//! Small process-wide operational counters. Exposed only through the authenticated API.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
pub static ACCEPTED: AtomicU64 = AtomicU64::new(0);
pub static REJECTED: AtomicU64 = AtomicU64::new(0);
pub static TICKS: AtomicU64 = AtomicU64::new(0);
pub static STEP_US: AtomicU64 = AtomicU64::new(0);
pub static MAX_STEP_US: AtomicU64 = AtomicU64::new(0);
pub static MAX_DELAY_US: AtomicU64 = AtomicU64::new(0);
pub fn snapshot() -> serde_json::Value {
    serde_json::json!({"commands_accepted": ACCEPTED.load(Relaxed), "commands_rejected": REJECTED.load(Relaxed),
        "ticks": TICKS.load(Relaxed), "step_total_us": STEP_US.load(Relaxed),
        "step_max_us": MAX_STEP_US.load(Relaxed), "scheduling_delay_max_us": MAX_DELAY_US.load(Relaxed)})
}
pub struct StepTimer(std::time::Instant);
impl Default for StepTimer {
    fn default() -> Self {
        Self(std::time::Instant::now())
    }
}
impl Drop for StepTimer {
    fn drop(&mut self) {
        let us = self.0.elapsed().as_micros() as u64;
        TICKS.fetch_add(1, Relaxed);
        STEP_US.fetch_add(us, Relaxed);
        MAX_STEP_US.fetch_max(us, Relaxed);
    }
}
