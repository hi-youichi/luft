//! Scheduler configuration (§2.2).

use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Adaptive default concurrency: 2× available cores, clamped to [4, 16].
fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() * 2)
        .unwrap_or(8)
        .clamp(4, 16)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    /// Semaphore permits (global concurrency ceiling).
    pub max_concurrency: usize,
    /// Per-run agent total ceiling (guards against runaway fan-out).
    pub quota_per_run: u32,
    pub retry: RetryPolicy,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_concurrency: default_concurrency(),
            quota_per_run: 1000,
            retry: RetryPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicy {
    /// Max retries (0 = no retry).
    pub max_attempts: u32,
    pub initial_backoff: Duration,
    pub backoff_multiplier: f64,
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 2,
            initial_backoff: Duration::from_millis(500),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(10),
        }
    }
}

impl RetryPolicy {
    /// Exponential backoff for the given retry attempt (1-based), capped.
    /// Cancellation is checked by the caller while sleeping.
    pub fn backoff(&self, attempt: u32) -> Duration {
        let exp = attempt.saturating_sub(1) as i32;
        let secs = self.initial_backoff.as_secs_f64() * self.backoff_multiplier.powi(exp);
        Duration::from_secs_f64(secs.min(self.max_backoff.as_secs_f64()))
    }
}
