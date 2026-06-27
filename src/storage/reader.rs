//! UI-ready query API.
//!
//! All functions read from the global SQLite DB and return structs that map
//! directly to UI components (run list, conversation view, run tree).

use crate::core::contract::ids::{AgentId, RunId};
use crate::storage::error::StorageResult;
use crate::storage::DbPool;
use sqlx::Row;

/// Run summary for list views.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunSummary {
    pub run_id: RunId,
    pub task: String,
    pub status: String,
    pub started_ts: String,
    pub finished_ts: Option<String>,
    pub elapsed_ms: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Per-agent overview within a run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentOverview {
    pub agent_id: AgentId,
    pub phase_id: Option<i64>,
    pub model: Option<String>,
    pub status: String,
    pub prompt_preview: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub elapsed_ms: i64,
    pub started_ts: String,
    pub done_ts: Option<String>,
}

/// One row of an agent's conversation stream.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnRow {
    pub seq: i64,
    pub ts: String,
    pub kind: String,
    pub role: Option<String>,
    pub text: Option<String>,
    pub tool_call_id: Option<String>,
    pub name: Option<String>,
    pub input: Option<String>,
    pub output: Option<String>,
    pub tool_status: Option<String>,
    pub file_path: Option<String>,
    pub file_op: Option<String>,
    pub diff: Option<String>,
}

/// Aggregated overview of a run.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunOverview {
    pub run: RunSummary,
    pub agents: Vec<AgentOverview>,
    pub turn_counts: Vec<TurnKindCount>,
    pub total_messages: i64,
    pub total_tool_calls: i64,
    pub total_file_edits: i64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnKindCount {
    pub kind: String,
    pub count: i64,
}

/// Span row used to build the run orchestration tree.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SpanRow {
    pub span_id: i64,
    pub kind: String,
    pub phase_id: Option<i64>,
    pub parent_span_id: Option<i64>,
    pub label: Option<String>,
    pub path: Option<String>,
    pub items: Option<i64>,
    pub max_rounds: Option<i64>,
    pub rounds: Option<i64>,
    pub converged: Option<i64>,
    pub ok: Option<i64>,
    pub failed: Option<i64>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub started_ts: Option<String>,
    pub done_ts: Option<String>,
    pub elapsed_ms: i64,
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// List recent runs, newest first. Paginated.
pub async fn list_runs(pool: &DbPool, limit: i64, offset: i64) -> StorageResult<Vec<RunSummary>> {
    let rows = sqlx::query(
        r#"SELECT run_id, task, status, started_ts, finished_ts, elapsed_ms,
                  input_tokens, output_tokens
           FROM runs
           ORDER BY started_ts DESC
           LIMIT ? OFFSET ?"#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|r| {
            Ok(RunSummary {
                run_id: r.try_get("run_id")?,
                task: r.try_get("task")?,
                status: r.try_get("status")?,
                started_ts: r.try_get("started_ts")?,
                finished_ts: r.try_get("finished_ts")?,
                elapsed_ms: r.try_get("elapsed_ms")?,
                input_tokens: r.try_get("input_tokens")?,
                output_tokens: r.try_get("output_tokens")?,
            })
        })
        .collect()
}

/// List turns for an agent, in seq order. Paginated.
pub async fn get_agent_turns(
    pool: &DbPool,
    run_id: RunId,
    agent_id: AgentId,
    limit: i64,
    offset: i64,
) -> StorageResult<Vec<TurnRow>> {
    let rows = sqlx::query(
        r#"SELECT seq, ts, kind, role, text, tool_call_id, name, input, output,
                  tool_status, file_path, file_op, diff
           FROM turns
           WHERE run_id = ? AND agent_id = ?
           ORDER BY seq
           LIMIT ? OFFSET ?"#,
    )
    .bind(run_id)
    .bind(agent_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|r| {
            Ok(TurnRow {
                seq: r.try_get("seq")?,
                ts: r.try_get("ts")?,
                kind: r.try_get("kind")?,
                role: r.try_get("role")?,
                text: r.try_get("text")?,
                tool_call_id: r.try_get("tool_call_id")?,
                name: r.try_get("name")?,
                input: r.try_get("input")?,
                output: r.try_get("output")?,
                tool_status: r.try_get("tool_status")?,
                file_path: r.try_get("file_path")?,
                file_op: r.try_get("file_op")?,
                diff: r.try_get("diff")?,
            })
        })
        .collect()
}

/// Single-agent overview (status, tokens, model).
pub async fn get_agent_overview(
    pool: &DbPool,
    run_id: RunId,
    agent_id: AgentId,
) -> StorageResult<AgentOverview> {
    let r = sqlx::query(
        r#"SELECT agent_id, phase_id, model, status, prompt_preview,
                  input_tokens, output_tokens, elapsed_ms, started_ts, done_ts
           FROM agents WHERE run_id = ? AND agent_id = ?"#,
    )
    .bind(run_id)
    .bind(agent_id)
    .fetch_one(pool)
    .await?;

    Ok(AgentOverview {
        agent_id: r.try_get("agent_id")?,
        phase_id: r.try_get("phase_id")?,
        model: r.try_get("model")?,
        status: r.try_get("status")?,
        prompt_preview: r.try_get("prompt_preview")?,
        input_tokens: r.try_get("input_tokens")?,
        output_tokens: r.try_get("output_tokens")?,
        elapsed_ms: r.try_get("elapsed_ms")?,
        started_ts: r.try_get("started_ts")?,
        done_ts: r.try_get("done_ts")?,
    })
}

/// All agents for a run.
pub async fn get_run_agents(
    pool: &DbPool,
    run_id: RunId,
) -> StorageResult<Vec<AgentOverview>> {
    let rows = sqlx::query(
        r#"SELECT agent_id, phase_id, model, status, prompt_preview,
                  input_tokens, output_tokens, elapsed_ms, started_ts, done_ts
           FROM agents
           WHERE run_id = ?
           ORDER BY started_ts"#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|r| {
            Ok(AgentOverview {
                agent_id: r.try_get("agent_id")?,
                phase_id: r.try_get("phase_id")?,
                model: r.try_get("model")?,
                status: r.try_get("status")?,
                prompt_preview: r.try_get("prompt_preview")?,
                input_tokens: r.try_get("input_tokens")?,
                output_tokens: r.try_get("output_tokens")?,
                elapsed_ms: r.try_get("elapsed_ms")?,
                started_ts: r.try_get("started_ts")?,
                done_ts: r.try_get("done_ts")?,
            })
        })
        .collect()
}

/// Aggregated run overview with turn counts.
pub async fn get_run_overview(pool: &DbPool, run_id: RunId) -> StorageResult<RunOverview> {
    let run_row = sqlx::query(
        r#"SELECT run_id, task, status, started_ts, finished_ts, elapsed_ms,
                  input_tokens, output_tokens
           FROM runs WHERE run_id = ?"#,
    )
    .bind(run_id)
    .fetch_one(pool)
    .await?;

    let run = RunSummary {
        run_id: run_row.try_get("run_id")?,
        task: run_row.try_get("task")?,
        status: run_row.try_get("status")?,
        started_ts: run_row.try_get("started_ts")?,
        finished_ts: run_row.try_get("finished_ts")?,
        elapsed_ms: run_row.try_get("elapsed_ms")?,
        input_tokens: run_row.try_get("input_tokens")?,
        output_tokens: run_row.try_get("output_tokens")?,
    };

    let agents = get_run_agents(pool, run_id).await?;

    let count_rows = sqlx::query(
        "SELECT kind, COUNT(*) AS c FROM turns WHERE run_id = ? GROUP BY kind",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    let mut turn_counts = Vec::new();
    let mut total_messages = 0i64;
    let mut total_tool_calls = 0i64;
    let mut total_file_edits = 0i64;
    for r in count_rows {
        let kind: String = r.try_get("kind")?;
        let count: i64 = r.try_get("c")?;
        match kind.as_str() {
            "message" => total_messages = count,
            "tool_call" | "tool_result" => total_tool_calls += count,
            "file_edit" => total_file_edits = count,
            _ => {}
        }
        turn_counts.push(TurnKindCount { kind, count });
    }

    Ok(RunOverview {
        run,
        agents,
        turn_counts,
        total_messages,
        total_tool_calls,
        total_file_edits,
    })
}

/// Orchestration spans for a run (caller assembles into a tree).
pub async fn get_run_spans(pool: &DbPool, run_id: RunId) -> StorageResult<Vec<SpanRow>> {
    let rows = sqlx::query(
        r#"SELECT span_id, kind, phase_id, parent_span_id, label, path,
                  items, max_rounds, rounds, converged, ok, failed,
                  result, error, started_ts, done_ts, elapsed_ms
           FROM spans WHERE run_id = ? ORDER BY span_id"#,
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|r| {
            Ok(SpanRow {
                span_id: r.try_get("span_id")?,
                kind: r.try_get("kind")?,
                phase_id: r.try_get("phase_id")?,
                parent_span_id: r.try_get("parent_span_id")?,
                label: r.try_get("label")?,
                path: r.try_get("path")?,
                items: r.try_get("items")?,
                max_rounds: r.try_get("max_rounds")?,
                rounds: r.try_get("rounds")?,
                converged: r.try_get("converged")?,
                ok: r.try_get("ok")?,
                failed: r.try_get("failed")?,
                result: r.try_get("result")?,
                error: r.try_get("error")?,
                started_ts: r.try_get("started_ts")?,
                done_ts: r.try_get("done_ts")?,
                elapsed_ms: r.try_get("elapsed_ms")?,
            })
        })
        .collect()
}

/// Alias for backwards-compat with the design doc.
pub async fn get_run_tree(pool: &DbPool, run_id: RunId) -> StorageResult<Vec<SpanRow>> {
    get_run_spans(pool, run_id).await
}

/// Search turns by free-text query (basic LIKE-based, sufficient for now;
/// will be upgraded to FTS5 in P4).
pub async fn search_turns(
    pool: &DbPool,
    run_id: RunId,
    query: &str,
    limit: i64,
) -> StorageResult<Vec<TurnRow>> {
    let like = format!("%{query}%");
    let rows = sqlx::query(
        r#"SELECT seq, ts, kind, role, text, tool_call_id, name, input, output,
                  tool_status, file_path, file_op, diff
           FROM turns
           WHERE run_id = ? AND text LIKE ?
           ORDER BY seq
           LIMIT ?"#,
    )
    .bind(run_id)
    .bind(like)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|r| {
            Ok(TurnRow {
                seq: r.try_get("seq")?,
                ts: r.try_get("ts")?,
                kind: r.try_get("kind")?,
                role: r.try_get("role")?,
                text: r.try_get("text")?,
                tool_call_id: r.try_get("tool_call_id")?,
                name: r.try_get("name")?,
                input: r.try_get("input")?,
                output: r.try_get("output")?,
                tool_status: r.try_get("tool_status")?,
                file_path: r.try_get("file_path")?,
                file_op: r.try_get("file_op")?,
                diff: r.try_get("diff")?,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::backend::AgentStatus;
    use crate::core::contract::event::{AgentEvent, ProgressDelta, RunStatus};
    use crate::core::contract::ids::TokenUsage;
    use crate::storage::db::open_db;
    use crate::storage::EventWriter;
    use chrono::Utc;
    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn setup() -> (tempfile::TempDir, DbPool, EventWriter) {
        let dir = tempdir().unwrap();
        let pool = open_db(&dir.path().join("test.db")).await.unwrap();
        let writer = EventWriter::new(pool.clone());
        (dir, pool, writer)
    }

    async fn seed_run_with_agent(
        w: &EventWriter,
        task: &str,
        agent_count: usize,
        msg_per_agent: usize,
    ) -> (RunId, Vec<AgentId>) {
        let run_id = uuid::Uuid::now_v7();
        w.write_event(&AgentEvent::RunStarted {
            run_id,
            task: task.into(),
            ts: Utc::now(),
        })
        .await
        .unwrap();

        let mut agents = Vec::new();
        for i in 0..agent_count {
            let agent_id = uuid::Uuid::now_v7();
            agents.push(agent_id);
            w.write_event(&AgentEvent::AgentStarted {
                run_id,
                phase_id: 0,
                agent_id,
                prompt_preview: format!("task {i}"),
                model: Some("claude-sonnet-4".into()),
                description: None,
                role: None,
                name: None,
                agent_seq: 0,
            })
            .await
            .unwrap();
            for j in 0..msg_per_agent {
                w.write_event(&AgentEvent::AgentProgress {
                    run_id,
                    agent_id,
                    delta: ProgressDelta::Message {
                        text: format!("agent {i} msg {j}"),
                    },
                })
                .await
                .unwrap();
            }
            w.write_event(&AgentEvent::AgentDone {
                run_id,
                agent_id,
                status: AgentStatus::Ok,
                tokens: TokenUsage {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                },
                elapsed_ms: 100,
                name: None,
                agent_seq: 0,
                output: serde_json::Value::Null,
                findings: Vec::new(),
                prompt: String::new(),
            })
            .await
            .unwrap();
        }

        w.write_event(&AgentEvent::RunDone {
            run_id,
            status: RunStatus::Completed,
            total_tokens: TokenUsage {
                input: 10,
                output: 5,
                cache_read: 0,
                cache_write: 0,
            },
            report: serde_json::json!({}),
        })
        .await
        .unwrap();

        (run_id, agents)
    }

    #[tokio::test]
    async fn list_runs_returns_seeded_runs() {
        let (_dir, pool, writer) = setup().await;
        for i in 0..3 {
            seed_run_with_agent(&writer, &format!("task {i}"), 1, 1).await;
        }

        let runs = list_runs(&pool, 10, 0).await.unwrap();
        assert_eq!(runs.len(), 3);
        assert!(runs.iter().all(|r| r.status == "completed"));
    }

    #[tokio::test]
    async fn list_runs_pagination() {
        let (_dir, pool, writer) = setup().await;
        for i in 0..5 {
            seed_run_with_agent(&writer, &format!("t{i}"), 1, 0).await;
        }

        let page1 = list_runs(&pool, 2, 0).await.unwrap();
        let page2 = list_runs(&pool, 2, 2).await.unwrap();
        let page3 = list_runs(&pool, 2, 4).await.unwrap();
        assert_eq!(page1.len(), 2);
        assert_eq!(page2.len(), 2);
        assert_eq!(page3.len(), 1);

        // Pages must not overlap.
        let ids: std::collections::HashSet<_> = page1
            .iter()
            .chain(&page2)
            .chain(&page3)
            .map(|r| r.run_id)
            .collect();
        assert_eq!(ids.len(), 5);
    }

    #[tokio::test]
    async fn get_agent_turns_preserves_order() {
        let (_dir, pool, writer) = setup().await;
        let (run_id, agents) = seed_run_with_agent(&writer, "t", 1, 4).await;
        let agent_id = agents[0];

        let turns = get_agent_turns(&pool, run_id, agent_id, 100, 0)
            .await
            .unwrap();
        assert_eq!(turns.len(), 4);
        let seqs: Vec<i64> = turns.iter().map(|t| t.seq).collect();
        assert!(seqs.windows(2).all(|w| w[0] < w[1]));
        for (i, t) in turns.iter().enumerate() {
            assert_eq!(t.kind, "message");
            assert_eq!(t.text.as_deref(), Some(format!("agent 0 msg {i}").as_str()));
        }
    }

    #[tokio::test]
    async fn get_agent_overview_returns_tokens() {
        let (_dir, pool, writer) = setup().await;
        let (run_id, agents) = seed_run_with_agent(&writer, "t", 1, 0).await;
        let agent_id = agents[0];

        let overview = get_agent_overview(&pool, run_id, agent_id).await.unwrap();
        assert_eq!(overview.status, "ok");
        assert_eq!(overview.input_tokens, 10);
        assert_eq!(overview.output_tokens, 5);
        assert_eq!(overview.model.as_deref(), Some("claude-sonnet-4"));
    }

    #[tokio::test]
    async fn get_run_overview_aggregates_turns() {
        let (_dir, pool, writer) = setup().await;
        let (run_id, _) = seed_run_with_agent(&writer, "t", 2, 3).await;

        let overview = get_run_overview(&pool, run_id).await.unwrap();
        assert_eq!(overview.run.task, "t");
        assert_eq!(overview.agents.len(), 2);
        assert_eq!(overview.total_messages, 6); // 2 agents × 3 messages
        assert_eq!(overview.turn_counts.len(), 1);
        assert_eq!(overview.turn_counts[0].kind, "message");
        assert_eq!(overview.turn_counts[0].count, 6);
    }

    #[tokio::test]
    async fn get_run_spans_returns_empty_when_no_spans() {
        let (_dir, pool, writer) = setup().await;
        let (run_id, _) = seed_run_with_agent(&writer, "t", 1, 1).await;

        let spans = get_run_spans(&pool, run_id).await.unwrap();
        assert!(spans.is_empty());
    }

    #[tokio::test]
    async fn search_turns_finds_matching_text() {
        let (_dir, pool, writer) = setup().await;
        let (run_id, agents) = seed_run_with_agent(&writer, "t", 1, 3).await;
        let agent_id = agents[0];

        // Add a distinctive message.
        writer
            .write_event(&AgentEvent::AgentProgress {
                run_id,
                agent_id,
                delta: ProgressDelta::Message {
                    text: "the quick brown fox jumps".into(),
                },
            })
            .await
            .unwrap();

        let results = search_turns(&pool, run_id, "fox", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].text.as_ref().unwrap().contains("fox"));
    }

    #[tokio::test]
    async fn file_edit_turns_appear_in_conversation() {
        let (_dir, pool, writer) = setup().await;
        let run_id = uuid::Uuid::now_v7();
        let agent_id = uuid::Uuid::now_v7();

        writer
            .write_event(&AgentEvent::AgentStarted {
                run_id,
                phase_id: 0,
                agent_id,
                prompt_preview: "".into(),
                model: None,
                description: None,
                role: None,
                name: None,
                agent_seq: 0,
            })
            .await
            .unwrap();

        writer
            .write_event(&AgentEvent::AgentProgress {
                run_id,
                agent_id,
                delta: ProgressDelta::FileEdit {
                    path: PathBuf::from("src/lib.rs"),
                },
            })
            .await
            .unwrap();

        let turns = get_agent_turns(&pool, run_id, agent_id, 100, 0)
            .await
            .unwrap();
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].kind, "file_edit");
        assert_eq!(turns[0].file_path.as_deref(), Some("src/lib.rs"));
    }
}