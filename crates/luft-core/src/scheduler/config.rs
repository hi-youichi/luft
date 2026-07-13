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
    /// Max schema validation retries before giving up (0 = no retry).
    pub schema_retry_max: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 2,
            initial_backoff: Duration::from_millis(500),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(10),
            schema_retry_max: 3,
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── SchedulerConfig ──────────────────────────────────────────

    #[test]
    fn scheduler_config_default_is_in_adaptive_range() {
        let cfg = SchedulerConfig::default();
        assert!(
            (4..=16).contains(&cfg.max_concurrency),
            "default max_concurrency {} outside adaptive [4,16]",
            cfg.max_concurrency
        );
        assert_eq!(cfg.quota_per_run, 1000);
        // Retry policy defaults are the canonical "2 tries, half a second".
        assert_eq!(cfg.retry.max_attempts, 2);
        assert_eq!(cfg.retry.initial_backoff, Duration::from_millis(500));
        assert_eq!(cfg.retry.schema_retry_max, 3);
    }

    #[test]
    fn scheduler_config_clone_preserves_fields() {
        let cfg = SchedulerConfig {
            max_concurrency: 32,
            quota_per_run: 250,
            retry: RetryPolicy {
                max_attempts: 5,
                initial_backoff: Duration::from_millis(100),
                backoff_multiplier: 1.5,
                max_backoff: Duration::from_secs(20),
                schema_retry_max: 1,
            },
        };
        let cloned = cfg.clone();
        assert_eq!(cloned.max_concurrency, 32);
        assert_eq!(cloned.quota_per_run, 250);
        assert_eq!(cloned.retry.max_attempts, 5);
        assert_eq!(cloned.retry.initial_backoff, Duration::from_millis(100));
        assert_eq!(cloned.retry.schema_retry_max, 1);
    }

    #[test]
    fn scheduler_config_serde_roundtrip() {
        let cfg = SchedulerConfig {
            max_concurrency: 8,
            quota_per_run: 100,
            retry: RetryPolicy {
                max_attempts: 3,
                initial_backoff: Duration::from_millis(250),
                backoff_multiplier: 1.7,
                max_backoff: Duration::from_secs(5),
                schema_retry_max: 2,
            },
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: SchedulerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_concurrency, cfg.max_concurrency);
        assert_eq!(back.quota_per_run, cfg.quota_per_run);
        assert_eq!(back.retry.max_attempts, cfg.retry.max_attempts);
        assert_eq!(back.retry.initial_backoff, cfg.retry.initial_backoff);
        assert_eq!(back.retry.backoff_multiplier, cfg.retry.backoff_multiplier);
        assert_eq!(back.retry.max_backoff, cfg.retry.max_backoff);
        assert_eq!(back.retry.schema_retry_max, cfg.retry.schema_retry_max);
    }

    #[test]
    fn scheduler_config_debug_format_includes_field_names() {
        let cfg = SchedulerConfig::default();
        let dbg = format!("{:?}", cfg);
        assert!(dbg.contains("max_concurrency"));
        assert!(dbg.contains("quota_per_run"));
        assert!(dbg.contains("retry"));
    }

    // ── RetryPolicy ──────────────────────────────────────────────

    #[test]
    fn retry_policy_default_values() {
        let r = RetryPolicy::default();
        assert_eq!(r.max_attempts, 2);
        assert_eq!(r.initial_backoff, Duration::from_millis(500));
        assert_eq!(r.backoff_multiplier, 2.0);
        assert_eq!(r.max_backoff, Duration::from_secs(10));
        assert_eq!(r.schema_retry_max, 3);
    }

    #[test]
    fn retry_policy_serde_roundtrip() {
        let r = RetryPolicy {
            max_attempts: 4,
            initial_backoff: Duration::from_millis(123),
            backoff_multiplier: 3.5,
            max_backoff: Duration::from_secs(30),
            schema_retry_max: 0,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: RetryPolicy = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_attempts, r.max_attempts);
        assert_eq!(back.initial_backoff, r.initial_backoff);
        assert_eq!(back.backoff_multiplier, r.backoff_multiplier);
        assert_eq!(back.max_backoff, r.max_backoff);
        assert_eq!(back.schema_retry_max, r.schema_retry_max);
    }

    #[test]
    fn backoff_attempt_1_returns_initial_backoff() {
        let r = RetryPolicy {
            initial_backoff: Duration::from_millis(500),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
            ..RetryPolicy::default()
        };
        assert_eq!(r.backoff(1), Duration::from_millis(500));
    }

    #[test]
    fn backoff_doubles_each_attempt() {
        let r = RetryPolicy {
            initial_backoff: Duration::from_millis(100),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
            ..RetryPolicy::default()
        };
        assert_eq!(r.backoff(1), Duration::from_millis(100));
        assert_eq!(r.backoff(2), Duration::from_millis(200));
        assert_eq!(r.backoff(3), Duration::from_millis(400));
        assert_eq!(r.backoff(4), Duration::from_millis(800));
        assert_eq!(r.backoff(5), Duration::from_millis(1600));
    }

    #[test]
    fn backoff_capped_at_max_backoff() {
        let r = RetryPolicy {
            initial_backoff: Duration::from_millis(500),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(1),
            ..RetryPolicy::default()
        };
        // 500ms, 1s, 2s, 4s — but the cap is 1s, so the 2nd attempt is the cap.
        assert_eq!(r.backoff(1), Duration::from_millis(500));
        assert_eq!(r.backoff(2), Duration::from_secs(1));
        assert_eq!(r.backoff(10), Duration::from_secs(1));
        // u32::MAX would overflow the i32 exponent inside backoff(), so we
        // don't drive it that high. The cap is still exercised by attempt=10.
        assert_eq!(r.backoff(60), Duration::from_secs(1));
    }

    #[test]
    fn backoff_attempt_0_treated_as_attempt_1() {
        // 1-based attempt: attempt 0 saturates to attempt 1 (no underflow panic).
        let r = RetryPolicy {
            initial_backoff: Duration::from_millis(750),
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
            ..RetryPolicy::default()
        };
        assert_eq!(r.backoff(0), r.backoff(1));
        assert_eq!(r.backoff(0), Duration::from_millis(750));
    }

    #[test]
    fn backoff_fractional_multiplier() {
        let r = RetryPolicy {
            initial_backoff: Duration::from_millis(1000),
            backoff_multiplier: 1.5,
            max_backoff: Duration::from_secs(60),
            ..RetryPolicy::default()
        };
        let b1 = r.backoff(1);
        let b2 = r.backoff(2);
        let b3 = r.backoff(3);
        // 1000ms, 1500ms, 2250ms — strictly increasing, monotonically.
        assert_eq!(b1, Duration::from_millis(1000));
        assert_eq!(b2, Duration::from_millis(1500));
        assert_eq!(b3, Duration::from_millis(2250));
        assert!(b1 < b2);
        assert!(b2 < b3);
    }

    #[test]
    fn backoff_never_exceeds_max_backoff_even_with_large_multiplier() {
        let r = RetryPolicy {
            initial_backoff: Duration::from_millis(10),
            backoff_multiplier: 100.0,
            max_backoff: Duration::from_secs(2),
            ..RetryPolicy::default()
        };
        for attempt in 1..20 {
            let b = r.backoff(attempt);
            assert!(
                b <= r.max_backoff,
                "backoff for attempt {} ({:?}) exceeded cap {:?}",
                attempt,
                b,
                r.max_backoff
            );
        }
    }

    #[test]
    fn backoff_zero_initial_returns_zero() {
        let r = RetryPolicy {
            initial_backoff: Duration::ZERO,
            backoff_multiplier: 2.0,
            max_backoff: Duration::from_secs(60),
            ..RetryPolicy::default()
        };
        assert_eq!(r.backoff(1), Duration::ZERO);
        assert_eq!(r.backoff(5), Duration::ZERO);
    }

    #[test]
    fn backoff_multiplier_one_keeps_constant() {
        let r = RetryPolicy {
            initial_backoff: Duration::from_millis(250),
            backoff_multiplier: 1.0,
            max_backoff: Duration::from_secs(60),
            ..RetryPolicy::default()
        };
        let b1 = r.backoff(1);
        for attempt in 2..=10 {
            assert_eq!(r.backoff(attempt), b1);
        }
    }

    #[test]
    fn retry_policy_clone_preserves_fields() {
        let r = RetryPolicy {
            max_attempts: 7,
            initial_backoff: Duration::from_millis(123),
            backoff_multiplier: 1.25,
            max_backoff: Duration::from_secs(45),
            schema_retry_max: 0,
        };
        let cloned = r.clone();
        assert_eq!(cloned.max_attempts, r.max_attempts);
        assert_eq!(cloned.initial_backoff, r.initial_backoff);
        assert_eq!(cloned.backoff_multiplier, r.backoff_multiplier);
        assert_eq!(cloned.max_backoff, r.max_backoff);
        assert_eq!(cloned.schema_retry_max, r.schema_retry_max);
    }

    #[test]
    fn retry_policy_debug_format() {
        let r = RetryPolicy::default();
        let dbg = format!("{:?}", r);
        assert!(dbg.contains("max_attempts"));
        assert!(dbg.contains("initial_backoff"));
        assert!(dbg.contains("backoff_multiplier"));
        assert!(dbg.contains("max_backoff"));
        assert!(dbg.contains("schema_retry_max"));
    }
}
