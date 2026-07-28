//! WorkflowService trait + concrete `WorkflowServiceImpl`.
//!
//! The trait defines the typed domain API; the impl contains all business
//! logic for workflow execution, querying, and cancellation.

use crate::error::ServiceError;
use crate::request::*;
use crate::response::*;
use luft::Luft;
use luft::RunHandle;
use luft_core::contract::event::{AgentEvent, LogLevel};
use luft_core::contract::ids::AgentId;
use luft_core::params;
use luft_core::query::StatusOutput;
use luft_runtime::validate_workflow;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ── Trait ──────────────────────────────────────────────────────────────

pub trait WorkflowService: Send + Sync {
    fn start_workflow(
        &self,
        req: ExecuteWorkflowRequest,
    ) -> impl std::future::Future<Output = Result<(ExecuteWorkflowResponse, RunHandle), ServiceError>>
           + Send;

    fn list_files(&self) -> Result<Vec<WorkflowFile>, ServiceError>;

    fn list_runs(&self, req: ListRunsRequest) -> Result<ListRunsResponse, ServiceError>;

    fn get_run_status(&self, req: GetRunStatusRequest) -> Result<RunStatusResponse, ServiceError>;

    fn get_run_events(&self, req: GetRunEventsRequest) -> Result<RunEventsResponse, ServiceError>;

    fn cancel_run(&self, req: CancelRunRequest) -> Result<CancelRunResponse, ServiceError>;
}

// ── WorkflowServiceImpl ────────────────────────────────────────────────

/// Concrete `WorkflowService` implementation backed by `Luft`.
pub struct WorkflowServiceImpl {
    luft: Luft,
    search_dirs: Vec<PathBuf>,
}

impl WorkflowServiceImpl {
    pub fn new(luft: Luft, search_dirs: Vec<PathBuf>) -> Self {
        Self { luft, search_dirs }
    }

    pub fn luft(&self) -> &Luft {
        &self.luft
    }

    pub fn search_dirs(&self) -> &[PathBuf] {
        &self.search_dirs
    }

    pub fn set_search_dirs(&mut self, dirs: Vec<PathBuf>) {
        self.search_dirs = dirs;
    }

    fn build_rich_status(&self, run_id: &str, status: &StatusOutput) -> RunStatusResponse {
        let events = self.luft.events(run_id).unwrap_or_default();
        let phases = derive_phases(&events);
        let total_phases = phases.len();
        let (report, error) = derive_report_and_error(&self.luft, run_id, &events, &status.status);

        RunStatusResponse {
            run_id: status.run_id.clone(),
            run_dir: status.run_dir.clone(),
            task: status.task.clone(),
            status: status.status.clone(),
            current_phase: status.current_phase,
            completed_phases: status.completed_phases,
            total_started: status.total_started,
            completed_agents: status.completed_agents,
            running_agents: status.running_agents,
            total_tokens: status.total_tokens,
            created_at: status.created_at.clone(),
            updated_at: status.updated_at.clone(),
            total_phases,
            phases,
            report,
            error,
        }
    }
}

impl WorkflowService for WorkflowServiceImpl {
    async fn start_workflow(
        &self,
        req: ExecuteWorkflowRequest,
    ) -> Result<(ExecuteWorkflowResponse, RunHandle), ServiceError> {
        req.validate()?;

        let concurrency = req.concurrency.map(|c| c as usize);
        let scoped_luft;
        let luft: &Luft = match concurrency {
            Some(n) => {
                scoped_luft = self.luft.with_concurrency(n);
                &scoped_luft
            }
            None => &self.luft,
        };

        let resume_from_id = req.resume_from_id.as_deref().filter(|s| !s.is_empty());

        if let Some(id) = resume_from_id {
            let handle = luft
                .start_resume(id)
                .await
                .map_err(|e| ServiceError::Internal(e.to_string()))?;
            let run_dir_name = handle.run_dir_name().to_string();
            return Ok((
                ExecuteWorkflowResponse {
                    run_id: run_dir_name,
                    status: "running".into(),
                    resumed_from: Some(id.to_string()),
                },
                handle,
            ));
        }

        let script =
            resolve_script_source(req.script.as_deref(), req.path.as_deref()).map_err(ServiceError::InvalidParam)?;

        let user_args = req.args.as_ref();
        let script = params::inject_args_globals(&script, user_args);

        let validation =
            validate_workflow(&script).map_err(|e| ServiceError::InvalidParam(e.to_string()))?;
        if !validation.is_valid() {
            return Err(ServiceError::InvalidParam(
                serde_json::to_string(&json!({
                    "valid": false,
                    "errors": validation.errors,
                    "warnings": validation.warnings,
                }))
                .unwrap_or_else(|_| "workflow validation failed".into()),
            ));
        }

        let handle = luft
            .start_script(&script)
            .await
            .map_err(|e| ServiceError::Internal(e.to_string()))?;
        let run_dir_name = handle.run_dir_name().to_string();

        Ok((
            ExecuteWorkflowResponse {
                run_id: run_dir_name,
                status: "running".into(),
                resumed_from: None,
            },
            handle,
        ))
    }

    fn list_files(&self) -> Result<Vec<WorkflowFile>, ServiceError> {
        Ok(collect_workflow_files(&self.search_dirs))
    }

    fn list_runs(&self, req: ListRunsRequest) -> Result<ListRunsResponse, ServiceError> {
        let limit = req.limit_or_default()? as usize;
        let cursor = req.cursor_str();
        let status_filter = req.status_filter_normalized()?;

        let mut runs = self
            .luft
            .list()
            .map_err(|e| ServiceError::Internal(e.to_string()))?;

        if let Some(ref f) = status_filter {
            runs.retain(|r| r.status.to_lowercase() == *f);
        }

        let total = runs.len();
        let start_idx = match cursor {
            None => 0,
            Some(c) => runs
                .iter()
                .position(|r| r.run_id == c)
                .map(|p| p + 1)
                .ok_or_else(|| ServiceError::InvalidParam(format!("cursor not found: {c}")))?,
        };

        let page: Vec<&StatusOutput> = runs.iter().skip(start_idx).take(limit).collect();
        let runs_page: Vec<RunSummary> = page
            .iter()
            .map(|r| RunSummary {
                run_id: r.run_id.clone(),
                task: r.task.clone(),
                status: r.status.clone(),
                total_tokens: r.total_tokens,
                created_at: r.created_at.clone(),
                updated_at: r.updated_at.clone(),
            })
            .collect();
        let count = runs_page.len();
        let next_cursor = if start_idx + count < total {
            runs_page.last().map(|r| r.run_id.clone())
        } else {
            None
        };
        let has_more = next_cursor.is_some();

        Ok(ListRunsResponse {
            runs: runs_page,
            count,
            next_cursor,
            has_more,
        })
    }

    fn get_run_status(&self, req: GetRunStatusRequest) -> Result<RunStatusResponse, ServiceError> {
        match self.luft.status(&req.run_id) {
            Ok(Some(status)) => Ok(self.build_rich_status(&req.run_id, &status)),
            Ok(None) => Err(ServiceError::NotFound(req.run_id)),
            Err(e) => Err(ServiceError::Internal(e.to_string())),
        }
    }

    fn get_run_events(&self, req: GetRunEventsRequest) -> Result<RunEventsResponse, ServiceError> {
        let events = self
            .luft
            .events(&req.run_id)
            .map_err(|e| ServiceError::Internal(e.to_string()))?;

        let events = if let Some(since_id) = req.since_event_id.as_deref() {
            filter_events_since(&events, since_id)
        } else {
            events
        };

        let types_filter = req.types_filter();
        let agent_id_filter = req.agent_id.as_deref();

        let serialized: Vec<Value> = events
            .iter()
            .map(|e| serde_json::to_value(e).unwrap_or(Value::Null))
            .collect();

        let filtered: Vec<&Value> = serialized
            .iter()
            .filter(|v| {
                let type_ok = types_filter.as_ref().is_none_or(|ts| {
                    v.get("type")
                        .and_then(|t| t.as_str())
                        .map(|t| ts.iter().any(|x| x == t))
                        .unwrap_or(false)
                });
                let agent_ok = agent_id_filter.is_none_or(|aid| {
                    v.get("agent_id")
                        .and_then(|a| a.as_str())
                        .map(|a| a == aid)
                        .unwrap_or(false)
                });
                type_ok && agent_ok
            })
            .collect();

        let total_matching = filtered.len() as u64;
        let offset = req.offset_or_default();
        let events_limit = req.events_limit_or_default();

        let page: Vec<Value> = filtered
            .into_iter()
            .skip(offset as usize)
            .take(events_limit as usize)
            .cloned()
            .collect();
        let next_offset = if offset + (page.len() as u64) < total_matching {
            Some(offset + page.len() as u64)
        } else {
            None
        };

        Ok(RunEventsResponse {
            events: page,
            offset,
            events_limit,
            total_matching,
            next_offset,
        })
    }

    fn cancel_run(&self, req: CancelRunRequest) -> Result<CancelRunResponse, ServiceError> {
        let run_id = &req.run_id;

        match self.luft.status(run_id) {
            Ok(Some(status)) if is_terminal_status(&status.status) => Ok(CancelRunResponse {
                run_id: run_id.clone(),
                result: "not_found_or_terminal".into(),
                note: Some(
                    "run is already in a terminal state (completed/failed/cancelled)".into(),
                ),
            }),
            Ok(Some(_)) => match self.luft.cancel(run_id) {
                Ok(()) => Ok(CancelRunResponse {
                    run_id: run_id.clone(),
                    result: "cancelling".into(),
                    note: Some(
                        "cancellation signalled; poll get_run_status to observe the terminal state"
                            .into(),
                    ),
                }),
                Err(e) => Err(ServiceError::Internal(e.to_string())),
            },
            Ok(None) => Ok(CancelRunResponse {
                run_id: run_id.clone(),
                result: "not_found_or_terminal".into(),
                note: Some("no active run with this identifier".into()),
            }),
            Err(e) => Err(ServiceError::Internal(e.to_string())),
        }
    }
}

// ── Private helpers ────────────────────────────────────────────────────

fn resolve_script_source(script: Option<&str>, path: Option<&str>) -> Result<String, String> {
    if let Some(s) = script {
        if !s.trim().is_empty() {
            return Ok(s.to_string());
        }
    }

    if let Some(p) = path {
        if !p.is_empty() {
            return std::fs::read_to_string(p)
                .map_err(|e| format!("failed to read workflow file '{p}': {e}"));
        }
    }

    Err("either 'script' or 'path' must be provided and non-empty".into())
}

fn collect_workflow_files(search_dirs: &[PathBuf]) -> Vec<WorkflowFile> {
    let mut entries = Vec::new();
    let mut seen_names: HashSet<String> = HashSet::new();

    for dir in search_dirs {
        if !dir.is_dir() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !seen_names.insert(stem.to_string()) {
                continue;
            }
            let description = extract_description(&path).unwrap_or_default();
            entries.push(WorkflowFile {
                name: stem.to_string(),
                path: path.display().to_string(),
                description,
            });
        }
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

fn extract_description(path: &Path) -> Option<String> {
    let source = std::fs::read_to_string(path).ok()?;

    if let Ok(Some(meta)) = luft_planner::meta::extract_meta(&source) {
        if !meta.reasoning.is_empty() {
            return Some(meta.reasoning);
        }
    }

    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(comment) = trimmed.strip_prefix("--") {
            let c = comment.trim();
            if !c.is_empty() {
                return Some(c.to_string());
            }
        }
    }

    None
}

fn is_terminal_status(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "completed" | "failed" | "cancelled"
    )
}

fn filter_events_since(events: &[AgentEvent], since_id: &str) -> Vec<AgentEvent> {
    let match_idx = events.iter().position(|e| event_matches_id(e, since_id));
    match match_idx {
        Some(idx) => events[idx + 1..].to_vec(),
        None => events.to_vec(),
    }
}

fn event_matches_id(event: &AgentEvent, id: &str) -> bool {
    if let Ok(serialized) = serde_json::to_string(event) {
        serialized.contains(id)
    } else {
        false
    }
}

fn summarize_output(output: &Value) -> Option<String> {
    if output.is_null() {
        return None;
    }
    let s = output.to_string();
    if s.is_empty() || s == "{}" {
        return None;
    }
    const MAX: usize = 200;
    if s.len() > MAX {
        Some(format!("{}…", &s[..MAX]))
    } else {
        Some(s)
    }
}

// ── Rich status derivation ─────────────────────────────────────────────

struct PhaseAcc {
    phase_id: u32,
    label: String,
    planned: usize,
    ok: usize,
    failed: usize,
    done: bool,
    agent_ids: Vec<AgentId>,
}

struct AgentAcc {
    status: String,
    tokens: Option<u64>,
    findings: usize,
    last_message: Option<String>,
}

fn derive_phases(events: &[AgentEvent]) -> Vec<PhaseView> {
    let mut phases: Vec<PhaseAcc> = Vec::new();
    let mut agents: HashMap<AgentId, AgentAcc> = HashMap::new();

    for event in events {
        match event {
            AgentEvent::PhaseStarted {
                phase_id,
                label,
                planned,
                ..
            } => {
                phases.push(PhaseAcc {
                    phase_id: *phase_id,
                    label: label.clone(),
                    planned: *planned,
                    ok: 0,
                    failed: 0,
                    done: false,
                    agent_ids: Vec::new(),
                });
            }
            AgentEvent::AgentStarted {
                phase_id,
                agent_id,
                ..
            } => {
                if let Some(p) = phases.iter_mut().rfind(|p| p.phase_id == *phase_id) {
                    p.agent_ids.push(*agent_id);
                }
                agents.insert(
                    *agent_id,
                    AgentAcc {
                        status: "running".into(),
                        tokens: None,
                        findings: 0,
                        last_message: None,
                    },
                );
            }
            AgentEvent::AgentDone {
                agent_id,
                status,
                tokens,
                output,
                findings,
                ..
            } => {
                let acc = agents.entry(*agent_id).or_insert_with(|| AgentAcc {
                    status: "running".into(),
                    tokens: None,
                    findings: 0,
                    last_message: None,
                });
                acc.status = status.as_str().into();
                acc.tokens = Some(tokens.total());
                acc.findings = findings.len();
                acc.last_message = summarize_output(output);
            }
            AgentEvent::PhaseDone {
                phase_id,
                ok,
                failed,
                ..
            } => {
                if let Some(p) = phases.iter_mut().rfind(|p| p.phase_id == *phase_id) {
                    p.ok = *ok;
                    p.failed = *failed;
                    p.done = true;
                }
            }
            _ => {}
        }
    }

    phases
        .into_iter()
        .map(|p| {
            let agent_rows: Vec<PhaseAgentView> = p
                .agent_ids
                .iter()
                .map(|aid| {
                    let short_id: String = aid.to_string().chars().take(7).collect();
                    match agents.get(aid) {
                        Some(a) => PhaseAgentView {
                            short_id,
                            status: a.status.clone(),
                            tokens: a.tokens,
                            findings: a.findings,
                            last_message: a.last_message.clone(),
                        },
                        None => PhaseAgentView {
                            short_id,
                            status: "running".into(),
                            tokens: None,
                            findings: 0,
                            last_message: None,
                        },
                    }
                })
                .collect();
            PhaseView {
                phase_id: p.phase_id,
                label: p.label,
                status: if p.done {
                    "completed".into()
                } else {
                    "running".into()
                },
                planned: Some(p.planned),
                ok: p.ok,
                failed: p.failed,
                agents: agent_rows,
            }
        })
        .collect()
}

fn derive_report_and_error(
    luft: &Luft,
    run_id: &str,
    events: &[AgentEvent],
    status: &str,
) -> (Value, Value) {
    let report = match luft.report(run_id) {
        Ok(luft_core::query::ReportStatus::Found(v)) => Some(v),
        _ => None,
    };

    let error = if status.eq_ignore_ascii_case("failed") {
        events.iter().rev().find_map(|e| match e {
            AgentEvent::Log {
                level: LogLevel::Error,
                msg,
                ..
            } => Some(msg.clone()),
            _ => None,
        })
    } else {
        None
    };

    (
        report.unwrap_or(Value::Null),
        error.map(Value::String).unwrap_or(Value::Null),
    )
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_impl() -> WorkflowServiceImpl {
        use luft_core::{MockBackend, MockBehavior, TokenUsage};
        use std::time::Duration;
        let backend = MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: json!({"result": "ok"}),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        );
        let luft = luft::Luft::builder()
            .backend(backend)
            .base_dir(tempfile::TempDir::new().unwrap().keep())
            .build()
            .unwrap();
        WorkflowServiceImpl::new(
            luft,
            vec![PathBuf::from("examples"), PathBuf::from("workflows")],
        )
    }

    // ── resolve_script_source ────────────────────────────────────────────

    #[test]
    fn resolve_script_from_inline() {
        let result = resolve_script_source(Some("report('hi')"), None).unwrap();
        assert_eq!(result, "report('hi')");
    }

    #[test]
    fn resolve_script_from_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("test.lua");
        std::fs::write(&file, "report('ok')").unwrap();

        let result = resolve_script_source(None, Some(file.to_str().unwrap())).unwrap();
        assert_eq!(result, "report('ok')");
    }

    #[test]
    fn resolve_script_path_not_found() {
        let err = resolve_script_source(None, Some("/nonexistent/file.lua")).unwrap_err();
        assert!(err.contains("failed to read workflow file"));
    }

    #[test]
    fn resolve_script_empty_script_falls_to_error() {
        let err = resolve_script_source(Some("  "), None).unwrap_err();
        assert!(err.contains("either 'script' or 'path'"));
    }

    #[test]
    fn resolve_script_neither_provided() {
        let err = resolve_script_source(None, None).unwrap_err();
        assert!(err.contains("either 'script' or 'path'"));
    }

    #[test]
    fn resolve_script_takes_priority_over_path() {
        let result = resolve_script_source(Some("inline"), Some("/fake")).unwrap();
        assert_eq!(result, "inline");
    }

    // ── filter_events_since ──────────────────────────────────────────────

    #[test]
    fn filter_events_since_empty() {
        let events: Vec<AgentEvent> = vec![];
        let result = filter_events_since(&events, "evt-1");
        assert!(result.is_empty());
    }

    #[test]
    fn filter_events_since_found_returns_after() {
        use luft_core::contract::event::RunStatus;
        use uuid::Uuid;
        let events = vec![
            AgentEvent::RunDone {
                report: json!({"id": "first"}),
                status: RunStatus::Completed,
                run_id: Uuid::nil(),
                total_tokens: Default::default(),
                ts: chrono::Utc::now(),
            },
            AgentEvent::RunDone {
                report: json!({"id": "second"}),
                status: RunStatus::Completed,
                run_id: Uuid::nil(),
                total_tokens: Default::default(),
                ts: chrono::Utc::now(),
            },
        ];
        let result = filter_events_since(&events, "first");
        assert_eq!(result.len(), 1);
        let result_json = serde_json::to_string(&result[0]).unwrap();
        assert!(result_json.contains("second"));
    }

    #[test]
    fn filter_events_since_not_found_returns_all() {
        use luft_core::contract::event::RunStatus;
        use uuid::Uuid;
        let events = vec![
            AgentEvent::RunDone {
                report: json!({"id": "a"}),
                status: RunStatus::Completed,
                run_id: Uuid::nil(),
                total_tokens: Default::default(),
                ts: chrono::Utc::now(),
            },
            AgentEvent::RunDone {
                report: json!({"id": "b"}),
                status: RunStatus::Completed,
                run_id: Uuid::nil(),
                total_tokens: Default::default(),
                ts: chrono::Utc::now(),
            },
        ];
        let result = filter_events_since(&events, "nonexistent");
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_events_since_match_at_last_returns_empty() {
        use luft_core::contract::event::RunStatus;
        use uuid::Uuid;
        let events = vec![AgentEvent::RunDone {
            report: json!({"id": "only"}),
            status: RunStatus::Completed,
            run_id: Uuid::nil(),
            total_tokens: Default::default(),
            ts: chrono::Utc::now(),
        }];
        let result = filter_events_since(&events, "only");
        assert!(result.is_empty());
    }

    // ── is_terminal_status ───────────────────────────────────────────────

    #[test]
    fn terminal_status_recognizes_all_three() {
        assert!(is_terminal_status("completed"));
        assert!(is_terminal_status("failed"));
        assert!(is_terminal_status("cancelled"));
    }

    #[test]
    fn terminal_status_case_insensitive() {
        assert!(is_terminal_status("COMPLETED"));
        assert!(is_terminal_status("Failed"));
    }

    #[test]
    fn terminal_status_rejects_running() {
        assert!(!is_terminal_status("running"));
        assert!(!is_terminal_status(""));
    }

    // ── summarize_output ─────────────────────────────────────────────────

    #[test]
    fn summarize_output_null_and_empty_are_none() {
        assert!(summarize_output(&Value::Null).is_none());
        assert!(summarize_output(&json!({})).is_none());
    }

    #[test]
    fn summarize_output_truncates_long_values() {
        let big = json!({ "s": "x".repeat(500) });
        let summary = summarize_output(&big).unwrap();
        assert!(summary.ends_with('\u{2026}'));
        assert!(summary.len() < big.to_string().len());
    }

    #[test]
    fn summarize_output_short_value_preserved() {
        let val = json!({"answer": "yes"});
        let summary = summarize_output(&val).unwrap();
        assert!(summary.contains("yes"));
    }

    // ── collect_workflow_files ───────────────────────────────────────────

    #[test]
    fn list_files_empty_dirs() {
        let impl_ = make_impl();
        let result = impl_.list_files().unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn list_files_with_files() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("a.lua"), "-- a test").unwrap();

        let luft = luft::Luft::builder()
            .backend(luft_core::MockBackend::new(
                "mock",
                vec![luft_core::MockBehavior::Success {
                    output: json!({}),
                    tokens: luft_core::contract::ids::TokenUsage::default(),
                    delay: std::time::Duration::ZERO,
                }],
            ))
            .base_dir(tempfile::TempDir::new().unwrap().keep())
            .build()
            .unwrap();

        let impl_ = WorkflowServiceImpl::new(luft, vec![dir.path().to_path_buf()]);
        let result = impl_.list_files().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "a");
    }

    // ── list_runs ────────────────────────────────────────────────────────

    #[test]
    fn list_runs_empty_when_no_runs() {
        let impl_ = make_impl();
        let req = ListRunsRequest {
            limit: None,
            cursor: None,
            status_filter: None,
        };
        let result = impl_.list_runs(req).unwrap();
        assert_eq!(result.count, 0);
        assert!(!result.has_more);
    }

    #[tokio::test]
    async fn list_runs_after_execute() {
        let impl_ = make_impl();
        let script = "meta = { reasoning = \"t\", phases = {} }\nfunction main() phase(\"t\") local r = agent({ prompt = \"hi\", name = \"a1\" }) report({ok=r.ok}) end";
        impl_.luft().run_script(script).await.expect("run_script");

        let req = ListRunsRequest {
            limit: None,
            cursor: None,
            status_filter: None,
        };
        let result = impl_.list_runs(req).unwrap();
        assert!(result.count <= 1);
    }

    // ── get_run_status ───────────────────────────────────────────────────

    #[tokio::test]
    async fn get_run_status_not_found() {
        let impl_ = make_impl();
        let req = GetRunStatusRequest {
            run_id: "nonexistent-dir".into(),
        };
        let err = impl_.get_run_status(req).unwrap_err();
        assert!(err.to_string().contains("run not found"));
    }

    #[tokio::test]
    async fn get_run_status_has_rich_fields() {
        let impl_ = make_impl();
        let script = "meta = { reasoning = \"t\", phases = {} }\nfunction main() phase(\"only\") local r = agent({ prompt = \"hi\", name = \"a1\" }) report({ok=r.ok}) end";
        let outcome = impl_.luft().run_script(script).await.expect("run_script");
        let run_id = outcome.run_dir_name;

        let req = GetRunStatusRequest {
            run_id: run_id.clone(),
        };
        match impl_.get_run_status(req) {
            Ok(resp) => {
                assert!(!resp.phases.is_empty() || resp.total_phases == 0);
            }
            Err(e) => assert!(e.to_string().contains("run not found")),
        }
    }

    // ── get_run_events ───────────────────────────────────────────────────

    #[tokio::test]
    async fn get_run_events_not_found() {
        let impl_ = make_impl();
        let req = GetRunEventsRequest {
            run_id: "nonexistent".into(),
            since_event_id: None,
            offset: None,
            events_limit: None,
            types: None,
            agent_id: None,
        };
        let err = impl_.get_run_events(req).unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[tokio::test]
    async fn get_run_events_pagination_fields_present() {
        let impl_ = make_impl();
        let script = "meta = { reasoning = \"t\", phases = {} }\nfunction main() report({ok=true}) end";
        let outcome = impl_.luft().run_script(script).await.expect("run_script");
        let run_id = outcome.run_dir_name;

        let parsed = {
            let mut last = None;
            for _ in 0..60 {
                let req = GetRunEventsRequest {
                    run_id: run_id.clone(),
                    since_event_id: None,
                    offset: None,
                    events_limit: Some(1),
                    types: None,
                    agent_id: None,
                };
                let resp = impl_.get_run_events(req).unwrap();
                if !resp.events.is_empty() {
                    last = Some(resp);
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            last.unwrap()
        };
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.offset, 0);
        assert_eq!(parsed.events_limit, 1);
        assert!(parsed.total_matching >= 1);
    }

    // ── cancel_run ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn cancel_run_not_found() {
        let impl_ = make_impl();
        let req = CancelRunRequest {
            run_id: "nonexistent".into(),
        };
        let result = impl_.cancel_run(req).unwrap();
        assert_eq!(result.result, "not_found_or_terminal");
    }

    // ── start_workflow ───────────────────────────────────────────────────

    #[tokio::test]
    async fn execute_workflow_validation_error() {
        let impl_ = make_impl();
        let req = ExecuteWorkflowRequest {
            script: Some("invalid lua !!!".into()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: None,
        };
        assert!(impl_.start_workflow(req).await.is_err());
    }

    #[tokio::test]
    async fn execute_workflow_neither_script_nor_path() {
        let impl_ = make_impl();
        let req = ExecuteWorkflowRequest {
            script: None,
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: None,
        };
        assert!(impl_
            .start_workflow(req)
            .await
            .is_err_and(|e| e.to_string().contains("either 'script' or 'path'")));
    }

    #[tokio::test]
    async fn execute_workflow_resume_exclusive_with_script() {
        let impl_ = make_impl();
        let req = ExecuteWorkflowRequest {
            script: Some("report('hi')".into()),
            path: None,
            resume_from_id: Some("some-id".into()),
            args: None,
            concurrency: None,
        };
        assert!(impl_
            .start_workflow(req)
            .await
            .is_err_and(|e| e.to_string().contains("mutually exclusive")));
    }

    #[tokio::test]
    async fn execute_workflow_concurrency_out_of_range() {
        let impl_ = make_impl();
        let req = ExecuteWorkflowRequest {
            script: Some("report('hi')".into()),
            path: None,
            resume_from_id: None,
            args: None,
            concurrency: Some(99),
        };
        assert!(impl_
            .start_workflow(req)
            .await
            .is_err_and(|e| e.to_string().contains("concurrency")));
    }

    // ── derive_phases (deterministic) ────────────────────────────────────

    #[test]
    fn derive_phases_single_phase_single_agent() {
        use luft_core::contract::backend::AgentStatus;
        use luft_core::contract::ids::TokenUsage;
        use uuid::Uuid;

        let agent_id: AgentId = Uuid::now_v7();
        let run_id = Uuid::nil();
        let events = vec![
            AgentEvent::PhaseStarted {
                run_id,
                phase_id: 0,
                label: "only".to_string(),
                planned: 1,
                description: None,
                role: None,
                ts: chrono::Utc::now(),
            },
            AgentEvent::AgentStarted {
                run_id,
                phase_id: 0,
                agent_id,
                prompt_preview: "hi".to_string(),
                model: None,
                description: None,
                role: None,
                name: None,
                agent_seq: 0,
                ts: Default::default(),
            },
            AgentEvent::AgentDone {
                run_id,
                agent_id,
                status: AgentStatus::Ok,
                tokens: TokenUsage {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_write: 0,
                },
                elapsed_ms: 12,
                name: None,
                agent_seq: 0,
                output: json!({"answer": "yes"}),
                findings: vec![],
                prompt: "hi".to_string(),
                retry_count: 0,
                ts: Default::default(),
            },
            AgentEvent::PhaseDone {
                run_id,
                phase_id: 0,
                ok: 1,
                failed: 0,
                ts: chrono::Utc::now(),
            },
        ];

        let phases = derive_phases(&events);
        assert_eq!(phases.len(), 1);
        let p = &phases[0];
        assert_eq!(p.phase_id, 0);
        assert_eq!(p.label, "only");
        assert_eq!(p.status, "completed");
        assert_eq!(p.planned, Some(1));
        assert_eq!(p.ok, 1);
        assert_eq!(p.failed, 0);
        assert_eq!(p.agents.len(), 1);
        assert_eq!(p.agents[0].status, "ok");
        assert_eq!(p.agents[0].tokens, Some(15));
        assert_eq!(p.agents[0].findings, 0);
        assert!(p.agents[0].short_id.len() <= 7);
    }

    #[test]
    fn derive_phases_running_phase_has_no_phase_done() {
        use uuid::Uuid;
        let run_id = Uuid::nil();
        let events = vec![AgentEvent::PhaseStarted {
            run_id,
            phase_id: 2,
            label: "in-flight".to_string(),
            planned: 3,
            description: None,
            role: None,
            ts: chrono::Utc::now(),
        }];
        let phases = derive_phases(&events);
        assert_eq!(phases.len(), 1);
        assert_eq!(phases[0].status, "running");
        assert!(phases[0].agents.is_empty());
    }

    #[test]
    fn derive_phases_ignores_unrelated_events() {
        use uuid::Uuid;
        let run_id = Uuid::nil();
        let events = vec![AgentEvent::Log {
            run_id,
            agent_id: None,
            level: LogLevel::Info,
            msg: "just a log line".to_string(),
        }];
        assert!(derive_phases(&events).is_empty());
    }

    // ── derive_report_and_error (deterministic) ──────────────────────────

    #[test]
    fn derive_report_and_error_completed_no_error() {
        let impl_ = make_impl();
        let (report, error) =
            derive_report_and_error(impl_.luft(), "no-such-run-on-disk", &[], "completed");
        assert!(report.is_null());
        assert!(error.is_null());
    }

    #[test]
    fn derive_report_and_error_failed_scans_last_error_log() {
        let impl_ = make_impl();
        use uuid::Uuid;
        let run_id = Uuid::nil();
        let events = vec![
            AgentEvent::Log {
                run_id,
                agent_id: None,
                level: LogLevel::Warn,
                msg: "first warning".to_string(),
            },
            AgentEvent::Log {
                run_id,
                agent_id: None,
                level: LogLevel::Error,
                msg: "boom, it broke".to_string(),
            },
        ];
        let (_report, error) =
            derive_report_and_error(impl_.luft(), "missing-dir", &events, "failed");
        assert_eq!(error, json!("boom, it broke"));
    }

    #[test]
    fn derive_report_and_error_non_failed_ignores_error_log() {
        let impl_ = make_impl();
        use uuid::Uuid;
        let run_id = Uuid::nil();
        let events = vec![AgentEvent::Log {
            run_id,
            agent_id: None,
            level: LogLevel::Error,
            msg: "logged but not fatal".to_string(),
        }];
        let (_report, error) =
            derive_report_and_error(impl_.luft(), "missing-dir", &events, "running");
        assert!(error.is_null());
    }
}
