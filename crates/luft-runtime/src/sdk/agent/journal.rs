//! Shared journal / resume plumbing for `agent()` and `parallel()`.
//!
//! Both primitives reduce a result down to the same four display parts and,
//! when a journal is present, record completed runs / honour cached ones. This
//! module owns those conversions so the two registrars stay focused on their
//! control flow.

use luft_core::contract::backend::AgentResult;
use luft_core::contract::finding::Finding;
use luft_core::contract::ids::{AgentId, PhaseId};
use luft_core::journal::{AgentCacheKey, JournalStore};
use luft_core::state::AgentResultCache;
use std::sync::Arc;

/// Display parts of an agent result — `(status, output, tokens, findings)` —
/// the exact tuple [`build_result_table`](crate::sdk::task::build_result_table)
/// consumes.
pub(super) type Slot = (String, serde_json::Value, u64, Vec<Finding>);

/// Reduce an owned scheduler result to its display parts.
pub(super) fn slot_from_result(result: AgentResult) -> Slot {
    (
        result.status.as_str().to_string(),
        result.output,
        result.tokens_used.total(),
        result.findings,
    )
}

/// Reduce a cached (resumed) result to its display parts. The cache already
/// stores `status` as a string and `tokens` as a total.
pub(super) fn slot_from_cache(cached: AgentResultCache) -> Slot {
    (cached.status, cached.output, cached.tokens, cached.findings)
}

/// Record a completed agent result into the journal. No-op when no journal is
/// configured.
pub(super) fn record(
    journal: &Option<Arc<JournalStore>>,
    cache_key: &AgentCacheKey,
    agent_id: AgentId,
    phase_id: PhaseId,
    result: &AgentResult,
) {
    if let Some(j) = journal {
        tracing::debug!(agent_id = %agent_id, phase_id, hash = &cache_key.hash[..8.min(cache_key.hash.len())], "recording result to journal");
        j.record_result(
            cache_key,
            agent_id,
            phase_id,
            result.status.clone(),
            result.output.clone(),
            result.findings.clone(),
            result.tokens_used,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luft_core::contract::backend::AgentStatus;
    use luft_core::contract::ids::TokenUsage;

    fn sample_result() -> AgentResult {
        AgentResult {
            agent_id: uuid::Uuid::now_v7(),
            status: AgentStatus::Ok,
            output: serde_json::json!({ "r": 1 }),
            findings: vec![],
            tokens_used: TokenUsage {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
            },
            artifacts: vec![],
            logs: Default::default(),
        }
    }

    #[test]
    fn slot_from_result_lowercases_status_and_totals_tokens() {
        let (status, output, tokens, findings) = slot_from_result(sample_result());
        assert_eq!(status, "ok");
        assert_eq!(output, serde_json::json!({ "r": 1 }));
        assert_eq!(tokens, 15); // total() = input + output
        assert!(findings.is_empty());
    }

    #[test]
    fn slot_from_cache_passes_fields_through() {
        let cached = AgentResultCache {
            agent_id: uuid::Uuid::now_v7(),
            phase_id: 1,
            status: "error".into(),
            output: serde_json::json!("x"),
            findings: vec![],
            tokens: 99,
            completed_at: 0,
            cache_key_hash: None,
            description: None,
            role: None,
        };
        let (status, output, tokens, _) = slot_from_cache(cached);
        assert_eq!(status, "error");
        assert_eq!(output, serde_json::json!("x"));
        assert_eq!(tokens, 99);
    }

#[test]
    fn record_without_journal_is_noop() {
        // Must not panic when no journal is configured.
        let key = AgentCacheKey::new("p", Some("m"), 1);
        record(&None, &key, uuid::Uuid::now_v7(), 1, &sample_result());
    }

    // ----------------------------------------------------------------------
    // Tests for the F5 contract — slot_from_result status formatting.
    //
    // The SDK reduces a scheduler result into a Slot tuple whose first
    // element is the persisted status string. Before F5 this was derived
    // from `format!("{:?}", status).to_lowercase()`, which collapsed
    // TimedOut → "timedout" (no underscore). The implementation must now
    // produce snake_case strings via `AgentStatus::as_str()` so the Slot
    // matches the AgentResultCache.status strings persisted by
    // RunStore/JournalStore.
    // ----------------------------------------------------------------------

    fn result_with_status(status: AgentStatus) -> AgentResult {
        AgentResult {
            agent_id: uuid::Uuid::now_v7(),
            status,
            output: serde_json::json!(null),
            findings: vec![],
            tokens_used: TokenUsage {
                input: 0,
                output: 0,
                cache_read: 0,
                cache_write: 0,
            },
            artifacts: vec![],
            logs: Default::default(),
        }
    }

    #[test]
    fn slot_from_result_uses_snake_case_status_for_each_variant() {
        // F5 KEY test: each variant must produce its `AgentStatus::as_str()`
        // value. This is the load-bearing invariant for parity with
        // RunStore::update_from_event → AgentResultCache.status.
        let cases: Vec<(AgentStatus, &str)> = vec![
            (AgentStatus::Ok, "ok"),
            (AgentStatus::Error, "error"),
            (AgentStatus::Cancelled, "cancelled"),
            (AgentStatus::TimedOut, "timed_out"),
        ];
        for (status, expected) in &cases {
            let (slot_status, _, _, _) = slot_from_result(result_with_status(status.clone()));
            assert_eq!(
                slot_status, *expected,
                "slot_from_result({status:?}) must produce snake_case status {expected:?}; \
                 got {slot_status:?}"
            );
        }
    }

    #[test]
    fn slot_from_result_timed_out_uses_underscore_not_collapsed() {
        // Strongest F5 regression guard for slot_from_result: TimedOut must
        // produce "timed_out" (snake_case) — NOT "timedout" (Debug lowercased).
        // The buggy form would silently break downstream consumers that
        // branch on the exact string "timed_out".
        let (slot_status, _, _, _) = slot_from_result(result_with_status(AgentStatus::TimedOut));
        assert_eq!(
            slot_status, "timed_out",
            "slot_from_result(TimedOut) must produce \"timed_out\"; got {slot_status:?}"
        );
        assert_ne!(
            slot_status, "timedout",
            "slot_from_result(TimedOut) must NOT collapse to Debug-lowercased \"timedout\""
        );
    }

    #[test]
    fn slot_from_result_ok_lowercases_and_totals_tokens() {
        // The existing happy-path test, kept as a baseline for the simplest
        // variant. The string here must be the same as `Ok.as_str()`.
        let (status, _, tokens, _) = slot_from_result(result_with_status(AgentStatus::Ok));
        assert_eq!(status, AgentStatus::Ok.as_str());
        assert_eq!(status, "ok");
        assert_eq!(tokens, 0);
    }

    #[test]
    fn slot_from_result_error_uses_snake_case() {
        let (status, _, _, _) = slot_from_result(result_with_status(AgentStatus::Error));
        assert_eq!(status, "error");
        // Distinguish from Buggy: should NOT be "Error" or any case-mismatched form.
        assert_ne!(status, "Error");
        assert_ne!(status, "ERROR");
    }

    #[test]
    fn slot_from_result_cancelled_uses_snake_case() {
        let (status, _, _, _) = slot_from_result(result_with_status(AgentStatus::Cancelled));
        assert_eq!(status, "cancelled");
        assert_eq!(status, AgentStatus::Cancelled.as_str());
    }

    #[test]
    fn slot_from_result_status_equals_as_str_for_all_variants() {
        // Property-style check: for every variant, the slot status string
        // must equal the canonical `as_str()` value exactly. This catches
        // any future divergence between the SDK's status formatting and the
        // contract layer's canonical mapping.
        let variants = [
            AgentStatus::Ok,
            AgentStatus::Error,
            AgentStatus::Cancelled,
            AgentStatus::TimedOut,
        ];
        for variant in &variants {
            let (slot_status, _, _, _) = slot_from_result(result_with_status(variant.clone()));
            assert_eq!(
                slot_status,
                variant.as_str(),
                "slot_from_result({variant:?}) must equal {variant:?}.as_str(); got {slot_status:?}"
            );
        }
    }

    #[test]
    fn slot_from_result_preserves_output_tokens_findings() {
        // The status change must not affect the other three slot fields.
        // Sanity guard against an over-zealous rewrite that reorders or
        // drops tuple elements.
        let r = AgentResult {
            agent_id: uuid::Uuid::now_v7(),
            status: AgentStatus::TimedOut,
            output: serde_json::json!({"value": 42}),
            findings: vec![],
            tokens_used: TokenUsage {
                input: 7,
                output: 11,
                cache_read: 3,
                cache_write: 5,
            },
            artifacts: vec![],
            logs: Default::default(),
        };
        let (status, output, tokens, findings) = slot_from_result(r);
        assert_eq!(status, "timed_out");
        assert_eq!(output, serde_json::json!({"value": 42}));
        assert_eq!(tokens, 7 + 11); // total() = input + output, excludes cache
        assert!(findings.is_empty());
    }
}
