//! `MockFileBackend` — file-backed deterministic backend for mock runs.
//!
//! Reads a `.mock.json` sidecar file and returns canned responses keyed by
//! `AgentTask::name`. Falls back to a `default` entry when the name doesn't
//! match. Tracks call statistics for post-run coverage reporting.

use crate::contract::*;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ── mock file format (serde) ───────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize)]
struct MockFile {
    #[serde(default)]
    #[allow(dead_code)]
    schema: Option<String>,
    responses: HashMap<String, MockEntry>,
    #[serde(default)]
    default: Option<MockEntry>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct MockEntry {
    #[serde(default = "default_ok")]
    status: String,
    output: serde_json::Value,
    #[serde(default)]
    tokens: TokenUsage,
    #[serde(default)]
    delay_ms: u64,
    #[serde(default)]
    findings: Vec<Finding>,
}

fn default_ok() -> String {
    "ok".to_string()
}

impl MockEntry {
    fn to_result(&self, agent_id: AgentId) -> AgentResult {
        let status = match self.status.as_str() {
            "ok" => AgentStatus::Ok,
            "error" => AgentStatus::Error,
            "cancelled" => AgentStatus::Cancelled,
            "timed_out" => AgentStatus::TimedOut,
            _ => AgentStatus::Ok,
        };
        AgentResult {
            agent_id,
            status,
            output: self.output.clone(),
            findings: self.findings.clone(),
            tokens_used: self.tokens,
            artifacts: vec![],
            logs: LogRef::default(),
            thread_id: None,
        }
    }

    fn delay(&self) -> Duration {
        Duration::from_millis(self.delay_ms)
    }
}

// ── runtime statistics ─────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct MockStats {
    total_calls: AtomicU32,
    matched: AtomicU32,
    fallback: AtomicU32,
    unmatched_names: Mutex<HashSet<String>>,
}

impl MockStats {
    pub fn snapshot(&self) -> MockStatsSnapshot {
        MockStatsSnapshot {
            total_calls: self.total_calls.load(Ordering::Relaxed),
            matched: self.matched.load(Ordering::Relaxed),
            fallback: self.fallback.load(Ordering::Relaxed),
            unmatched_names: self
                .unmatched_names
                .lock()
                .unwrap()
                .iter()
                .cloned()
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MockStatsSnapshot {
    pub total_calls: u32,
    pub matched: u32,
    pub fallback: u32,
    pub unmatched_names: Vec<String>,
}

impl MockStatsSnapshot {
    pub fn all_matched(&self) -> bool {
        self.fallback == 0 && self.total_calls > 0
    }

    pub fn coverage_pct(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.matched as f64 / self.total_calls as f64 * 100.0
        }
    }
}

// ── backend ────────────────────────────────────────────────────────

pub struct MockFileBackend {
    responses: HashMap<String, MockEntry>,
    default: Option<MockEntry>,
    stats: Arc<MockStats>,
}

impl MockFileBackend {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("cannot read mock file '{}': {}", path.display(), e))?;
        Self::parse(&content)
    }

    pub fn parse(json: &str) -> anyhow::Result<Self> {
        let file: MockFile = serde_json::from_str(json).map_err(|e| {
            anyhow::anyhow!(
                "invalid mock JSON: {}. Expected format: {{\"responses\": {{...}}, \"default\": {{...}}}}",
                e
            )
        })?;
        Ok(Self {
            responses: file.responses,
            default: file.default,
            stats: Arc::new(MockStats::default()),
        })
    }

    pub fn stats_handle(&self) -> Arc<MockStats> {
        Arc::clone(&self.stats)
    }

    fn resolve(&self, task: &AgentTask) -> (MockEntry, bool) {
        if let Some(name) = task.name.as_deref() {
            if let Some(entry) = self.responses.get(name) {
                return (entry.clone(), true);
            }
            if let Some((_, entry)) = self.responses.iter().find(|(key, _)| {
                name.len() > key.len()
                    && name.starts_with(key.as_str())
                    && name.as_bytes()[key.len()] == b' '
            }) {
                return (entry.clone(), true);
            }
        }
        let entry = self.default.clone().unwrap_or_else(|| MockEntry {
            status: "error".into(),
            output: serde_json::json!({
                "error": "mock: no matching response and no default configured"
            }),
            tokens: TokenUsage::default(),
            delay_ms: 0,
            findings: vec![],
        });
        (entry, false)
    }
}

#[async_trait::async_trait]
impl AgentBackend for MockFileBackend {
    fn id(&self) -> &'static str {
        "mockfile"
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities::default()
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError> {
        let (entry, matched) = self.resolve(&task);

        self.stats.total_calls.fetch_add(1, Ordering::Relaxed);
        if matched {
            self.stats.matched.fetch_add(1, Ordering::Relaxed);
        } else {
            self.stats.fallback.fetch_add(1, Ordering::Relaxed);
            if let Some(name) = &task.name {
                self.stats
                    .unmatched_names
                    .lock()
                    .unwrap()
                    .insert(name.clone());
            }
        }

        let delay = entry.delay();
        if !delay.is_zero() {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = ctx.cancel.cancelled() => return Err(BackendError::Cancelled),
            }
        }

        Ok(entry.to_result(task.agent_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::backend::{AgentTask, RunContext};
    use std::path::PathBuf;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    fn make_task(name: Option<&str>) -> AgentTask {
        AgentTask {
            agent_id: Uuid::now_v7(),
            phase_id: 0,
            prompt: "test".into(),
            model: None,
            description: None,
            role: None,
            name: name.map(String::from),
            agent_seq: 0,
            allowlist: None,
            workdir: PathBuf::from("."),
            mcp_endpoint: None,
            timeout: None,
            output_schema: None,
            workdir_override: None,
            thread_id: None,
        }
    }

    fn make_ctx() -> RunContext {
        let (events, _) = tokio::sync::broadcast::channel(16);
        RunContext {
            run_id: Uuid::now_v7(),
            cancel: CancellationToken::new(),
            events,
        }
    }

    const MINIMAL_MOCK: &str = r#"{
        "responses": {
            "plan": {
                "output": {"text": "mock plan"},
                "tokens": {"input": 10, "output": 5}
            },
            "analyze": {
                "output": {"text": "mock analysis"},
                "tokens": {"input": 20, "output": 15},
                "status": "ok"
            }
        },
        "default": {
            "output": {"text": "fallback"},
            "tokens": {"input": 0, "output": 0}
        }
    }"#;

    #[test]
    fn parse_minimal_mock() {
        let mb = MockFileBackend::parse(MINIMAL_MOCK).unwrap();
        assert_eq!(mb.responses.len(), 2);
        assert!(mb.default.is_some());
    }

    #[test]
    fn parse_invalid_json_errors() {
        assert!(MockFileBackend::parse("not json").is_err());
    }

    #[test]
    fn parse_missing_responses_errors() {
        assert!(MockFileBackend::parse(r#"{"default": {"output": {}}}"#).is_err());
    }

    #[tokio::test]
    async fn run_matched_by_name() {
        let mb = MockFileBackend::parse(MINIMAL_MOCK).unwrap();
        let result = mb.run(make_task(Some("plan")), make_ctx()).await.unwrap();
        assert_eq!(result.status, AgentStatus::Ok);
        assert_eq!(result.output["text"], "mock plan");
    }

    #[tokio::test]
    async fn run_unmatched_uses_default() {
        let mb = MockFileBackend::parse(MINIMAL_MOCK).unwrap();
        let result = mb
            .run(make_task(Some("unknown")), make_ctx())
            .await
            .unwrap();
        assert_eq!(result.output["text"], "fallback");
    }

    #[tokio::test]
    async fn run_no_name_uses_default() {
        let mb = MockFileBackend::parse(MINIMAL_MOCK).unwrap();
        let result = mb.run(make_task(None), make_ctx()).await.unwrap();
        assert_eq!(result.output["text"], "fallback");
    }

    #[tokio::test]
    async fn run_no_default_no_match_returns_error() {
        let mb = MockFileBackend::parse(r#"{"responses": {"x": {"output": {}}}}"#).unwrap();
        let result = mb.run(make_task(Some("y")), make_ctx()).await.unwrap();
        assert_eq!(result.status, AgentStatus::Error);
    }

    #[tokio::test]
    async fn stats_track_matched_and_fallback() {
        let mb = MockFileBackend::parse(MINIMAL_MOCK).unwrap();
        let stats = mb.stats_handle();

        mb.run(make_task(Some("plan")), make_ctx()).await.unwrap();
        mb.run(make_task(Some("analyze")), make_ctx())
            .await
            .unwrap();
        mb.run(make_task(Some("nope")), make_ctx()).await.unwrap();
        mb.run(make_task(None), make_ctx()).await.unwrap();

        let snap = stats.snapshot();
        assert_eq!(snap.total_calls, 4);
        assert_eq!(snap.matched, 2);
        assert_eq!(snap.fallback, 2);
        assert!(snap.unmatched_names.contains(&"nope".to_string()));
        assert!(!snap.all_matched());
    }

    #[tokio::test]
    async fn stats_all_matched() {
        let mb = MockFileBackend::parse(MINIMAL_MOCK).unwrap();
        let stats = mb.stats_handle();

        mb.run(make_task(Some("plan")), make_ctx()).await.unwrap();
        mb.run(make_task(Some("analyze")), make_ctx())
            .await
            .unwrap();

        let snap = stats.snapshot();
        assert!(snap.all_matched());
        assert!((snap.coverage_pct() - 100.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn run_status_error() {
        let mb = MockFileBackend::parse(
            r#"{"responses": {"fail": {"output": {}, "status": "error"}}, "default": {"output": {}}}"#,
        )
        .unwrap();
        let result = mb.run(make_task(Some("fail")), make_ctx()).await.unwrap();
        assert_eq!(result.status, AgentStatus::Error);
    }

    #[tokio::test]
    async fn run_prefix_match() {
        let mb = MockFileBackend::parse(
            r#"{"responses": {"analyze": {"output": {"text": "mock analysis"}}}, "default": {"output": {}}}"#,
        )
        .unwrap();
        let result = mb
            .run(make_task(Some("analyze src/main.rs")), make_ctx())
            .await
            .unwrap();
        assert_eq!(result.output["text"], "mock analysis");
    }

    #[tokio::test]
    async fn run_exact_match_preferred_over_prefix() {
        let mb = MockFileBackend::parse(
            r#"{"responses": {
                "analyze": {"output": {"text": "prefix"}},
                "analyze special": {"output": {"text": "exact"}}
            }, "default": {"output": {}}}"#,
        )
        .unwrap();
        let result = mb
            .run(make_task(Some("analyze special")), make_ctx())
            .await
            .unwrap();
        assert_eq!(result.output["text"], "exact");
    }

    #[tokio::test]
    async fn run_delay_respects_cancel() {
        let mb = MockFileBackend::parse(
            r#"{"responses": {"slow": {"output": {}, "delay_ms": 5000}}, "default": {"output": {}}}"#,
        )
        .unwrap();
        let ctx = make_ctx();
        ctx.cancel.cancel();
        let err = mb.run(make_task(Some("slow")), ctx).await.unwrap_err();
        assert!(matches!(err, BackendError::Cancelled));
    }
}
