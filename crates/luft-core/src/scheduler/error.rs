//! Scheduler errors (§2.2).

use crate::contract::backend::BackendError;

#[derive(thiserror::Error, Debug)]
pub enum SchedulerError {
    #[error("unknown backend: {0}")]
    UnknownBackend(String),
    #[error("no backend registered")]
    NoBackendRegistered,
    #[error("run not initialized: {0}")]
    RunNotFound(crate::contract::RunId),
    #[error("quota exceeded: limit={limit}, used={used}")]
    QuotaExceeded { limit: u32, used: u32 },
    #[error("run cancelled")]
    RunCancelled,
    #[error("agent cancelled")]
    AgentCancelled,
    #[error("backend error (non-retryable): {0}")]
    NonRetryable(#[from] BackendError),
    #[error("backend error after {attempts} attempts: {source}")]
    Exhausted { attempts: u32, source: BackendError },
    #[error("output schema validation failed: {0}")]
    SchemaValidation(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::BackendError;
    use crate::contract::RunId;

    // ── Display formatting (spec-required strings) ──────────────

    #[test]
    fn display_unknown_backend_includes_id() {
        let e = SchedulerError::UnknownBackend("opencode".to_string());
        assert_eq!(e.to_string(), "unknown backend: opencode");
    }

    #[test]
    fn display_no_backend_registered_is_static() {
        let e = SchedulerError::NoBackendRegistered;
        assert_eq!(e.to_string(), "no backend registered");
    }

    #[test]
    fn display_run_not_found_includes_id() {
        let id = RunId::nil();
        let e = SchedulerError::RunNotFound(id);
        let s = e.to_string();
        assert!(s.starts_with("run not initialized: "), "got: {s}");
        assert!(s.contains(&id.to_string()), "missing uuid in: {s}");
    }

    #[test]
    fn display_quota_exceeded_uses_both_fields() {
        let e = SchedulerError::QuotaExceeded {
            limit: 10,
            used: 11,
        };
        assert_eq!(e.to_string(), "quota exceeded: limit=10, used=11");
    }

    #[test]
    fn display_run_cancelled_is_static() {
        assert_eq!(SchedulerError::RunCancelled.to_string(), "run cancelled");
    }

    #[test]
    fn display_agent_cancelled_is_static() {
        assert_eq!(
            SchedulerError::AgentCancelled.to_string(),
            "agent cancelled"
        );
    }

    #[test]
    fn display_non_retryable_includes_backend_message() {
        let inner = BackendError::Protocol("bad frame".to_string());
        let e = SchedulerError::NonRetryable(inner);
        assert_eq!(
            e.to_string(),
            "backend error (non-retryable): protocol error: bad frame"
        );
    }

    #[test]
    fn display_exhausted_uses_attempts_and_source() {
        let inner = BackendError::Timeout;
        let e = SchedulerError::Exhausted {
            attempts: 3,
            source: inner,
        };
        assert_eq!(
            e.to_string(),
            "backend error after 3 attempts: backend timed out"
        );
    }

    #[test]
    fn display_schema_validation_includes_reason() {
        let e = SchedulerError::SchemaValidation("missing field 'answer'".to_string());
        assert_eq!(
            e.to_string(),
            "output schema validation failed: missing field 'answer'"
        );
    }

    // ── Construction / variant payload access ───────────────────

    #[test]
    fn construct_unknown_backend() {
        let e = SchedulerError::UnknownBackend(String::from("claude"));
        match e {
            SchedulerError::UnknownBackend(s) => assert_eq!(s, "claude"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn construct_no_backend_registered() {
        let e = SchedulerError::NoBackendRegistered;
        assert!(matches!(e, SchedulerError::NoBackendRegistered));
    }

    #[test]
    fn construct_run_not_found() {
        let id = RunId::now_v7();
        match SchedulerError::RunNotFound(id) {
            SchedulerError::RunNotFound(got) => assert_eq!(got, id),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn construct_quota_exceeded() {
        let e = SchedulerError::QuotaExceeded {
            limit: 5,
            used: 5,
        };
        match e {
            SchedulerError::QuotaExceeded { limit, used } => {
                assert_eq!(limit, 5);
                assert_eq!(used, 5);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn construct_run_cancelled() {
        assert!(matches!(
            SchedulerError::RunCancelled,
            SchedulerError::RunCancelled
        ));
    }

    #[test]
    fn construct_agent_cancelled() {
        assert!(matches!(
            SchedulerError::AgentCancelled,
            SchedulerError::AgentCancelled
        ));
    }

    #[test]
    fn construct_non_retryable() {
        let inner = BackendError::Connection("refused".to_string());
        match SchedulerError::NonRetryable(inner) {
            SchedulerError::NonRetryable(got) => {
                assert!(matches!(got, BackendError::Connection(_)));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn construct_exhausted() {
        let inner = BackendError::Spawn("oops".to_string());
        let e = SchedulerError::Exhausted {
            attempts: 7,
            source: inner,
        };
        match e {
            SchedulerError::Exhausted { attempts, source } => {
                assert_eq!(attempts, 7);
                assert!(matches!(source, BackendError::Spawn(_)));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn construct_schema_validation() {
        let e = SchedulerError::SchemaValidation(String::from("type mismatch"));
        match e {
            SchedulerError::SchemaValidation(s) => assert_eq!(s, "type mismatch"),
            _ => panic!("wrong variant"),
        }
    }

    // ── `From<BackendError>` auto-conversion (via `#[from]`) ────

    #[test]
    fn from_backend_error_yields_non_retryable() {
        let inner = BackendError::Parse("json".to_string());
        let converted: SchedulerError = inner.into();
        assert!(matches!(
            converted,
            SchedulerError::NonRetryable(BackendError::Parse(_))
        ));
    }

    #[test]
    fn from_backend_error_preserves_message() {
        let inner = BackendError::Io("disk full".to_string());
        let converted: SchedulerError = inner.into();
        assert_eq!(
            converted.to_string(),
            "backend error (non-retryable): IO error: disk full"
        );
    }

    #[test]
    fn from_backend_error_question_mark_works_in_fallible_fn() {
        fn make_err() -> Result<(), SchedulerError> {
            let _ = BackendError::Timeout;
            Err(BackendError::Timeout)?;
            Ok(())
        }
        let err = make_err().unwrap_err();
        assert!(matches!(
            err,
            SchedulerError::NonRetryable(BackendError::Timeout)
        ));
    }

    // ── std::error::Error::source() chain ───────────────────────

    #[test]
    fn source_for_non_retryable_is_backend_error() {
        let inner = BackendError::Protocol("x".to_string());
        let e = SchedulerError::NonRetryable(inner);
        let src = std::error::Error::source(&e).expect("NonRetryable should expose a source");
        let downcast = src
            .downcast_ref::<BackendError>()
            .expect("source should be BackendError");
        assert!(matches!(downcast, BackendError::Protocol(_)));
    }

    #[test]
    fn source_for_exhausted_is_backend_error() {
        let inner = BackendError::Spawn("boom".to_string());
        let e = SchedulerError::Exhausted {
            attempts: 4,
            source: inner,
        };
        let src = std::error::Error::source(&e).expect("Exhausted should expose a source");
        let downcast = src
            .downcast_ref::<BackendError>()
            .expect("source should be BackendError");
        assert!(matches!(downcast, BackendError::Spawn(_)));
    }

    #[test]
    fn source_for_plain_variants_is_none() {
        let cases = [
            SchedulerError::UnknownBackend("x".to_string()),
            SchedulerError::NoBackendRegistered,
            SchedulerError::RunNotFound(RunId::nil()),
            SchedulerError::QuotaExceeded { limit: 0, used: 0 },
            SchedulerError::RunCancelled,
            SchedulerError::AgentCancelled,
            SchedulerError::SchemaValidation(String::new()),
        ];
        for e in cases {
            assert!(
                std::error::Error::source(&e).is_none(),
                "{e:?} should have no source"
            );
        }
    }

    // ── Trait surface (compile-time assertions) ────────────────

    #[test]
    fn scheduler_error_is_send_and_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<SchedulerError>();
        assert_sync::<SchedulerError>();
    }

    #[test]
    fn scheduler_error_implements_std_error() {
        fn assert_error<T: std::error::Error + 'static>() {}
        assert_error::<SchedulerError>();
    }

    // ── Debug formatting ────────────────────────────────────────

    #[test]
    fn debug_all_variants_contains_variant_name() {
        let cases: Vec<(SchedulerError, &str)> = vec![
            (
                SchedulerError::UnknownBackend("id".to_string()),
                "UnknownBackend",
            ),
            (
                SchedulerError::NoBackendRegistered,
                "NoBackendRegistered",
            ),
            (
                SchedulerError::RunNotFound(RunId::nil()),
                "RunNotFound",
            ),
            (
                SchedulerError::QuotaExceeded {
                    limit: 1,
                    used: 2,
                },
                "QuotaExceeded",
            ),
            (SchedulerError::RunCancelled, "RunCancelled"),
            (SchedulerError::AgentCancelled, "AgentCancelled"),
            (
                SchedulerError::NonRetryable(BackendError::Timeout),
                "NonRetryable",
            ),
            (
                SchedulerError::Exhausted {
                    attempts: 1,
                    source: BackendError::Timeout,
                },
                "Exhausted",
            ),
            (
                SchedulerError::SchemaValidation("x".to_string()),
                "SchemaValidation",
            ),
        ];
        for (e, expected) in cases {
            let dbg = format!("{e:?}");
            assert!(
                dbg.contains(expected),
                "Debug of {expected} did not contain variant name: {dbg}"
            );
        }
    }

    // ── Edge cases / boundary conditions ────────────────────────

    #[test]
    fn unknown_backend_with_empty_string() {
        let e = SchedulerError::UnknownBackend(String::new());
        assert_eq!(e.to_string(), "unknown backend: ");
    }

    #[test]
    fn unknown_backend_with_unicode_and_special_chars() {
        let e = SchedulerError::UnknownBackend("后端-1/💥".to_string());
        let s = e.to_string();
        assert!(s.starts_with("unknown backend: "));
        assert!(s.contains("后端-1/💥"));
    }

    #[test]
    fn run_not_found_with_nil_uuid() {
        let e = SchedulerError::RunNotFound(RunId::nil());
        assert!(e.to_string().contains(&RunId::nil().to_string()));
    }

    #[test]
    fn run_not_found_with_random_uuid() {
        let id = RunId::now_v7();
        let e = SchedulerError::RunNotFound(id);
        assert!(e.to_string().contains(&id.to_string()));
    }

    #[test]
    fn quota_exceeded_with_zero_limit_and_zero_used() {
        let e = SchedulerError::QuotaExceeded {
            limit: 0,
            used: 0,
        };
        assert_eq!(e.to_string(), "quota exceeded: limit=0, used=0");
    }

    #[test]
    fn quota_exceeded_with_max_u32_values() {
        let e = SchedulerError::QuotaExceeded {
            limit: u32::MAX,
            used: u32::MAX,
        };
        assert_eq!(
            e.to_string(),
            "quota exceeded: limit=4294967295, used=4294967295"
        );
    }

    #[test]
    fn quota_exceeded_with_used_greater_than_limit() {
        let e = SchedulerError::QuotaExceeded {
            limit: 3,
            used: 100,
        };
        let s = e.to_string();
        assert!(s.contains("limit=3"));
        assert!(s.contains("used=100"));
    }

    #[test]
    fn exhausted_with_zero_attempts_is_valid() {
        let e = SchedulerError::Exhausted {
            attempts: 0,
            source: BackendError::Cancelled,
        };
        assert_eq!(
            e.to_string(),
            "backend error after 0 attempts: cancelled"
        );
    }

    #[test]
    fn exhausted_with_max_attempts() {
        let e = SchedulerError::Exhausted {
            attempts: u32::MAX,
            source: BackendError::Timeout,
        };
        let s = e.to_string();
        assert!(s.contains("4294967295 attempts"));
        assert!(s.contains("backend timed out"));
    }

    #[test]
    fn exhausted_preserves_inner_backend_error() {
        let inner = BackendError::Execution("nope".to_string());
        let e = SchedulerError::Exhausted {
            attempts: 2,
            source: inner,
        };
        match e {
            SchedulerError::Exhausted { source, .. } => match source {
                BackendError::Execution(msg) => assert_eq!(msg, "nope"),
                _ => panic!("inner error lost"),
            },
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn non_retryable_for_each_backend_variant_via_from() {
        let cases: Vec<BackendError> = vec![
            BackendError::Spawn("s".to_string()),
            BackendError::Protocol("p".to_string()),
            BackendError::Connection("c".to_string()),
            BackendError::Timeout,
            BackendError::Cancelled,
            BackendError::Config("cfg".to_string()),
            BackendError::Io("io".to_string()),
            BackendError::Parse("parse".to_string()),
            BackendError::Execution("exec".to_string()),
            BackendError::Other(anyhow::anyhow!("anyhow-err")),
        ];
        for inner in cases {
            let debug_repr = format!("{inner:?}");
            let converted: SchedulerError = inner.into();
            assert!(
                matches!(converted, SchedulerError::NonRetryable(_)),
                "From conversion failed for {debug_repr}"
            );
            let s = converted.to_string();
            assert!(
                s.starts_with("backend error (non-retryable): "),
                "wrong display prefix for {debug_repr}: {s}"
            );
        }
    }

    #[test]
    fn schema_validation_with_empty_string() {
        let e = SchedulerError::SchemaValidation(String::new());
        assert_eq!(e.to_string(), "output schema validation failed: ");
    }

    #[test]
    fn schema_validation_with_multiline_message() {
        let msg = "line one\nline two\n  indented".to_string();
        let e = SchedulerError::SchemaValidation(msg);
        let s = e.to_string();
        assert!(s.starts_with("output schema validation failed: "));
        assert!(s.contains("line one"));
        assert!(s.contains("line two"));
        assert!(s.contains("indented"));
    }

    // ── Pattern matching coverage ───────────────────────────────
    //
    // If a new variant is added, the match below becomes non-exhaustive and
    // the build breaks, prompting test updates.

    #[test]
    fn exhaustive_match_counts_all_variants() {
        let variants: Vec<SchedulerError> = vec![
            SchedulerError::UnknownBackend(String::new()),
            SchedulerError::NoBackendRegistered,
            SchedulerError::RunNotFound(RunId::nil()),
            SchedulerError::QuotaExceeded {
                limit: 0,
                used: 0,
            },
            SchedulerError::RunCancelled,
            SchedulerError::AgentCancelled,
            SchedulerError::NonRetryable(BackendError::Timeout),
            SchedulerError::Exhausted {
                attempts: 1,
                source: BackendError::Timeout,
            },
            SchedulerError::SchemaValidation(String::new()),
        ];
        let mut seen = std::collections::HashSet::new();
        for v in variants {
            let key = match &v {
                SchedulerError::UnknownBackend(_) => "UnknownBackend",
                SchedulerError::NoBackendRegistered => "NoBackendRegistered",
                SchedulerError::RunNotFound(_) => "RunNotFound",
                SchedulerError::QuotaExceeded { .. } => "QuotaExceeded",
                SchedulerError::RunCancelled => "RunCancelled",
                SchedulerError::AgentCancelled => "AgentCancelled",
                SchedulerError::NonRetryable(_) => "NonRetryable",
                SchedulerError::Exhausted { .. } => "Exhausted",
                SchedulerError::SchemaValidation(_) => "SchemaValidation",
            };
            assert!(seen.insert(key), "duplicate variant: {key}");
        }
        assert_eq!(seen.len(), 9, "expected 9 distinct variants");
    }
}
