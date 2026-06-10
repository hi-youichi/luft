//! Shared journal / resume plumbing for `agent()` and `parallel()`.
//!
//! Both primitives reduce a result down to the same four display parts and,
//! when a journal is present, record completed runs / honour cached ones. This
//! module owns those conversions so the two registrars stay focused on their
//! control flow.

use crate::core::contract::backend::AgentResult;
use crate::core::contract::finding::Finding;
use crate::core::contract::ids::{AgentId, PhaseId};
use crate::core::journal::{AgentCacheKey, JournalStore};
use crate::core::state::AgentResultCache;
use std::sync::Arc;

/// Display parts of an agent result — `(status, output, tokens, findings)` —
/// the exact tuple [`build_result_table`](crate::runtime::sdk::task::build_result_table)
/// consumes.
pub(super) type Slot = (String, serde_json::Value, u64, Vec<Finding>);

/// Reduce an owned scheduler result to its display parts.
pub(super) fn slot_from_result(result: AgentResult) -> Slot {
    (
        format!("{:?}", result.status).to_lowercase(),
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
    use crate::core::contract::backend::AgentStatus;
    use crate::core::contract::ids::TokenUsage;

    fn sample_result() -> AgentResult {
        AgentResult {
            agent_id: uuid::Uuid::now_v7(),
            status: AgentStatus::Ok,
            output: serde_json::json!({ "r": 1 }),
            findings: vec![],
            tokens_used: TokenUsage { input: 10, output: 5, cache_read: 0, cache_write: 0 },
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
}
