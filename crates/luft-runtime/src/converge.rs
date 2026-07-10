#![allow(dead_code)]
//! Converge — adversarial verification and convergence logic.
//!
//! This module implements the core dynamic workflow convergence algorithm:
//! 1. Producer agents generate findings from items
//! 2. Adversarial agents attempt to refute those findings
//! 3. Voting determines surviving findings
//! 4. Iteration continues until convergence or max rounds
//!
//! This is the key feature that distinguishes Dynamic Workflows from simple
//! parallel execution: agents verify each other's work before the result
//! reaches the user.

use luft_core::contract::backend::{AgentStatus, RunContext};
use luft_core::contract::event::AgentEvent;
use luft_core::contract::finding::Finding;
use luft_core::Scheduler;
use crate::error::ScriptError;
use crate::sdk::SdkContext;
use mlua::{Lua, Table, Value};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::sync::Arc;

/// Configuration for convergence verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvergeConfig {
    /// Whether to enable adversarial verification.
    pub adversarial: bool,
    /// Voting threshold (0.0-1.0). Findings with approval >= threshold survive.
    pub vote_threshold: f32,
    /// Maximum number of verification rounds.
    pub max_rounds: u32,
    /// Number of producer agents per round.
    pub producers_per_item: u32,
    /// Number of adversarial agents per finding.
    pub adversaries_per_finding: u32,
    /// Model to use for agents.
    pub model: Option<String>,
}

impl Default for ConvergeConfig {
    fn default() -> Self {
        Self {
            adversarial: true,
            vote_threshold: 0.7,
            max_rounds: 3,
            producers_per_item: 1,
            adversaries_per_finding: 1,
            model: None,
        }
    }
}

/// Result of a converge operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvergeResult {
    /// Items that survived verification.
    pub surviving_items: Vec<serde_json::Value>,
    /// All findings generated during verification.
    pub findings: Vec<Finding>,
    /// Number of rounds executed.
    pub rounds: u32,
    /// Whether convergence was achieved.
    pub converged: bool,
    /// Voting statistics per round.
    pub round_stats: Vec<RoundStats>,
}

/// Statistics for a single verification round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundStats {
    pub round: u32,
    pub items_input: usize,
    pub findings_generated: usize,
    pub findings_survived: usize,
    pub findings_refuted: usize,
    pub approval_rate: f32,
}

/// Internal state during convergence iteration.
struct ConvergeState {
    items: Vec<serde_json::Value>,
    findings: Vec<Finding>,
    round_stats: Vec<RoundStats>,
    converged: bool,
}

/// Execute adversarial convergence on a list of items.
///
/// This function:
/// 1. Generates findings for each item using producer agents
/// 2. Has adversarial agents attempt to refute each finding
/// 3. Votes on findings; survivors proceed to next round
/// 4. Repeats until convergence or max rounds
pub async fn execute_convergence(
    items: Vec<serde_json::Value>,
    producer_prompt: &str,
    adversary_prompt: &str,
    config: ConvergeConfig,
    scheduler: &Arc<Scheduler>,
    run_ctx: &RunContext,
) -> Result<ConvergeResult, ScriptError> {
    if items.is_empty() {
        tracing::debug!("converge: no items, returning empty");
        return Ok(ConvergeResult {
            surviving_items: vec![],
            findings: vec![],
            rounds: 0,
            converged: true,
            round_stats: vec![],
        });
    }

    tracing::info!(
        n_items = items.len(),
        max_rounds = config.max_rounds,
        adversarial = config.adversarial,
        "converge started"
    );

    let mut state = ConvergeState {
        items,
        findings: vec![],
        round_stats: vec![],
        converged: false,
    };

    for round in 1..=config.max_rounds {
        let items_count = state.items.len();
        tracing::info!(round, items_count, "converge round started");
        let round_input = state.items.clone();

        // Phase 1: Producer agents generate findings
        let (findings, _producer_stats) = generate_findings(
            &round_input,
            producer_prompt,
            config.producers_per_item,
            config.model.clone(),
            scheduler,
            run_ctx,
        )
        .await;

        let current_findings = findings.clone();
        state.findings.extend(current_findings);

        if findings.is_empty() {
            tracing::info!(round, "converge: no findings generated, ending");
            break;
        }

        // Phase 2: Adversarial agents refute findings (if enabled)
        let (surviving_findings, vote_stats) = if config.adversarial {
            tracing::debug!(
                round,
                n_findings = findings.len(),
                adversaries = config.adversaries_per_finding,
                "adversarial verification started"
            );
            verify_findings(
                &findings,
                adversary_prompt,
                config.adversaries_per_finding,
                config.vote_threshold,
                config.model.clone(),
                scheduler,
                run_ctx,
            )
            .await
        } else {
            (findings.clone(), VoteStats::default())
        };

        // Update round statistics
        let round_stats = RoundStats {
            round,
            items_input: items_count,
            findings_generated: findings.len(),
            findings_survived: surviving_findings.len(),
            findings_refuted: findings.len() - surviving_findings.len(),
            approval_rate: vote_stats.approval_rate,
        };
        tracing::info!(
            round,
            generated = findings.len(),
            survived = surviving_findings.len(),
            refuted = findings.len() - surviving_findings.len(),
            approval_rate = vote_stats.approval_rate,
            "converge round finished"
        );
        state.round_stats.push(round_stats);

        // Check for convergence
        if surviving_findings.is_empty() {
            tracing::info!(round, "converge: all findings refuted, converged");
            state.converged = true;
            break;
        }

        // Check if items unchanged (full convergence)
        if surviving_findings.len() == findings.len() && round > 1 {
            tracing::info!(round, "converge: no findings refuted, full convergence");
            state.converged = true;
            break;
        }

        // For next round, use surviving findings as items
        state.items = surviving_findings
            .into_iter()
            .map(|f| {
                serde_json::json!({
                    "kind": f.kind,
                    "severity": format!("{:?}", f.severity).to_lowercase(),
                    "title": f.title,
                    "detail": f.detail,
                    "location": f.location,
                    "evidence": f.evidence,
                    "data": f.data
                })
            })
            .collect();
    }

    tracing::info!(
        rounds = state.round_stats.len(),
        converged = state.converged,
        surviving = state.items.len(),
        total_findings = state.findings.len(),
        "converge finished"
    );
    Ok(ConvergeResult {
        surviving_items: state.items,
        findings: state.findings,
        rounds: state.round_stats.len() as u32,
        converged: state.converged,
        round_stats: state.round_stats,
    })
}

/// Generate findings for items using producer agents (parallel execution).
async fn generate_findings(
    items: &[serde_json::Value],
    prompt_template: &str,
    producers_per_item: u32,
    model: Option<String>,
    scheduler: &Arc<Scheduler>,
    run_ctx: &RunContext,
) -> (Vec<Finding>, ProducerStats) {
    // Build all tasks for parallel execution
    let mut tasks = Vec::new();
    for item in items {
        for _ in 0..producers_per_item {
            let prompt =
                prompt_template.replace("{item}", &serde_json::to_string(item).unwrap_or_default());
            let agent_id = uuid::Uuid::now_v7();
            let task = luft_core::contract::backend::AgentTask {
                agent_id,
                phase_id: 2, // Converge phase
                prompt,
                model: model.clone(),
                description: None,
                role: Some("producer".to_string()),
                name: None,
                agent_seq: 0,
                allowlist: None,
                workdir: std::path::PathBuf::from("."),
                mcp_endpoint: None,
                timeout: None,
                output_schema: None,
            };
            tasks.push((task, None::<String>));
        }
    }

    // Run all producers in parallel
    let results = scheduler.run_parallel(run_ctx.run_id, tasks).await;
    let agents_run = results.len();
    tracing::debug!(agents_run, "producer agents completed");
    let mut all_findings = Vec::new();

    for result in results.into_iter().flatten() {
        all_findings.extend(result.findings);
    }

    let stats = ProducerStats {
        items_processed: items.len(),
        agents_run,
        findings_generated: all_findings.len(),
    };

    (all_findings, stats)
}

/// Verify findings using adversarial agents.
async fn verify_findings(
    findings: &[Finding],
    prompt_template: &str,
    adversaries_per_finding: u32,
    vote_threshold: f32,
    model: Option<String>,
    scheduler: &Arc<Scheduler>,
    run_ctx: &RunContext,
) -> (Vec<Finding>, VoteStats) {
    let mut votes: Vec<(Finding, usize)> = findings.iter().map(|f| (f.clone(), 0)).collect();
    let total_votes_possible = findings.len() * adversaries_per_finding as usize;

    for finding in findings {
        let mut approval_count = 0usize;

        for _ in 0..adversaries_per_finding {
            let prompt = prompt_template.replace(
                "{finding}",
                &serde_json::to_string(finding).unwrap_or_default(),
            );
            let agent_id = uuid::Uuid::now_v7();

            let task = luft_core::contract::backend::AgentTask {
                agent_id,
                phase_id: 2,
                prompt,
                model: model.clone(),
                description: None,
                role: Some("adversary".to_string()),
                name: None,
                agent_seq: 0,
                allowlist: None,
                workdir: std::path::PathBuf::from("."),
                mcp_endpoint: None,
                timeout: None,
                output_schema: None,
            };

            let result = scheduler.run_agent(run_ctx.run_id, task, None).await;

            if let Ok(r) = result {
                if r.status == AgentStatus::Ok {
                    tracing::trace!(%agent_id, "adversary approved finding");
                    approval_count += 1;
                } else {
                    tracing::trace!(%agent_id, ?r.status, "adversary rejected finding");
                }
            }
        }

        if let Some((_, count)) = votes.iter_mut().find(|(f, _)| f.title == finding.title) {
            *count = approval_count;
        }
    }

    let approval_rate = if total_votes_possible > 0 {
        votes.iter().map(|(_, c)| *c).sum::<usize>() as f32 / total_votes_possible as f32
    } else {
        0.0
    };

    let surviving: Vec<Finding> = votes
        .into_iter()
        .filter(|(_, count)| {
            let threshold = (count * 100) / (adversaries_per_finding as usize).max(1);
            threshold as f32 >= vote_threshold * 100.0
        })
        .map(|(f, _)| f)
        .collect();

    tracing::debug!(
        surviving = surviving.len(),
        refuted = findings.len() - surviving.len(),
        approval_rate,
        "adversarial voting completed"
    );

    let stats = VoteStats { approval_rate };

    (surviving, stats)
}

#[derive(Debug, Default)]
#[allow(dead_code)] // diagnostic accounting; retained for future progress reporting
struct ProducerStats {
    items_processed: usize,
    agents_run: usize,
    findings_generated: usize,
}

#[derive(Debug, Default)]
struct VoteStats {
    approval_rate: f32,
}

// ============================================================================
// Lua SDK Bridge
// ============================================================================

/// Extract a string from mlua::String, converting to owned String to avoid borrow issues.
fn extract_string(s: mlua::String) -> Option<String> {
    s.to_str().ok().map(|s| s.to_string())
}

/// Register the converge SDK function in Lua.
///
/// `handle` is the tokio runtime handle used to block on the async scheduler.
/// It must be called from a blocking execution context (see `sandbox`).
pub fn register_converge_sdk(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();
    let sched = cx.scheduler.clone();
    let rc = cx.run_ctx.clone();
    let handle = cx.handle.clone();
    let events = cx.events();
    let run_id = cx.run_id();
    let phase_counter = cx.phase_counter.clone();
    let span_counter = cx.span_counter.clone();

    let converge_fn = lua.create_function(move |_lua, (items, options): (Table, Table)| {
        let scheduler = sched.clone();
        let run_ctx = rc.clone();
        let handle = handle.clone();
        // Parse options
        let config = parse_converge_options(&options);
        tracing::debug!(
            adversarial = config.adversarial,
            max_rounds = config.max_rounds,
            "converge SDK invoked"
        );
        let phase_id = phase_counter.load(Ordering::Relaxed);
        let span_id = span_counter.fetch_add(1, Ordering::Relaxed);
        let max_rounds = config.max_rounds;

        let producer_prompt = options
            .get::<mlua::String>("producer_prompt")
            .ok()
            .and_then(extract_string)
            .unwrap_or_else(|| {
                "Analyze the following item and report any findings: {item}".to_string()
            });

        let adversary_prompt = options
            .get::<mlua::String>("adversary_prompt")
            .ok()
            .and_then(extract_string)
            .unwrap_or_else(|| {
                "Review this finding and determine if it is valid or should be refuted: {finding}"
                    .to_string()
            });

        // Convert items to Vec
        let items_vec: Vec<serde_json::Value> = items
            .sequence_values()
            .filter_map(|v: mlua::Result<Value>| v.ok())
            .filter_map(|v| lua_value_to_json(&v).ok())
            .collect();

        let _ = events.send(AgentEvent::ConvergeStarted {
            run_id,
            phase_id,
            span_id,
            items: items_vec.len(),
            max_rounds,
        });
        let t0 = std::time::Instant::now();

        // Execute convergence by blocking on the shared scheduler runtime.
        let result = handle.block_on(execute_convergence(
            items_vec,
            &producer_prompt,
            &adversary_prompt,
            config,
            &scheduler,
            &run_ctx,
        ));
        let elapsed_ms = t0.elapsed().as_millis() as u64;

        match result {
            Ok(ref res) => {
                tracing::info!(
                    rounds = res.rounds,
                    converged = res.converged,
                    surviving = res.surviving_items.len(),
                    elapsed_ms,
                    "converge SDK completed"
                );
            }
            Err(ref e) => {
                tracing::error!(error = %e, elapsed_ms, "converge SDK failed");
            }
        }
        match result {
            Ok(res) => {
                let _ = events.send(AgentEvent::ConvergeDone {
                    run_id,
                    phase_id,
                    span_id,
                    rounds: res.rounds,
                    converged: res.converged,
                    surviving: res.surviving_items.len(),
                    result: serde_json::json!({
                        "surviving": res.surviving_items,
                        "rounds": res.rounds,
                        "converged": res.converged,
                        "findings": res.findings,
                    }),
                    elapsed_ms,
                    error: None,
                });
                let result_table = _lua.create_table()?;
                let surviving = _lua.create_table()?;
                for (i, item) in res.surviving_items.iter().enumerate() {
                    let lua_val = json_to_lua_value(_lua, item.clone())?;
                    surviving.set(i + 1, lua_val)?;
                }
                result_table.set("surviving", surviving)?;
                result_table.set("rounds", res.rounds)?;
                result_table.set("converged", res.converged)?;

                let findings_table = _lua.create_table()?;
                for (i, finding) in res.findings.iter().enumerate() {
                    let ft = _lua.create_table()?;
                    ft.set("kind", finding.kind.as_str())?;
                    ft.set("severity", format!("{:?}", finding.severity).to_lowercase())?;
                    ft.set("title", finding.title.as_str())?;
                    ft.set("detail", finding.detail.as_str())?;
                    findings_table.set(i + 1, ft)?;
                }
                result_table.set("findings", findings_table)?;

                Ok(result_table)
            }
            Err(e) => {
                let _ = events.send(AgentEvent::ConvergeDone {
                    run_id,
                    phase_id,
                    span_id,
                    rounds: 0,
                    converged: false,
                    surviving: 0,
                    result: serde_json::Value::Null,
                    elapsed_ms,
                    error: Some(e.to_string()),
                });
                Err(mlua::Error::RuntimeError(format!("converge error: {}", e)))
            }
        }
    })?;

    globals.set("converge", converge_fn)?;
    Ok(())
}

/// Parse converge options from a Lua table.
fn parse_converge_options(options: &Table) -> ConvergeConfig {
    let mut config = ConvergeConfig::default();

    if let Ok(adversarial) = options.get::<mlua::String>("adversarial") {
        let s = extract_string(adversarial).unwrap_or_else(|| "true".to_string());
        config.adversarial = s != "false";
    }
    if let Ok(threshold) = options.get::<mlua::Number>("vote_threshold") {
        config.vote_threshold = threshold as f32;
    }
    if let Ok(max_rounds) = options.get::<mlua::Integer>("max_rounds") {
        config.max_rounds = max_rounds as u32;
    }
    if let Ok(producers) = options.get::<mlua::Integer>("producers") {
        config.producers_per_item = producers as u32;
    }
    if let Ok(adversaries) = options.get::<mlua::Integer>("adversaries") {
        config.adversaries_per_finding = adversaries as u32;
    }
    if let Ok(model) = options.get::<mlua::String>("model") {
        let s = extract_string(model).unwrap_or_default();
        config.model = if s.is_empty() { None } else { Some(s) };
    }

    config
}

/// Convert a Lua Value to serde_json::Value.
fn lua_value_to_json(value: &Value) -> Result<serde_json::Value, mlua::Error> {
    match value {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        Value::Integer(i) => Ok(serde_json::Value::Number(serde_json::Number::from(*i))),
        Value::Number(n) => {
            if let Some(n) = serde_json::Number::from_f64(*n) {
                Ok(serde_json::Value::Number(n))
            } else {
                Ok(serde_json::Value::Null)
            }
        }
        Value::String(s) => {
            let owned = s.clone();
            let s = match owned.to_str() {
                Ok(s) => s.to_string(),
                Err(_) => return Ok(serde_json::Value::Null),
            };
            Ok(serde_json::Value::String(s))
        }
        Value::Table(t) => {
            // Try as array first
            let len = t.len().unwrap_or(0);
            if len > 0 {
                let arr: Vec<serde_json::Value> = t
                    .sequence_values()
                    .filter_map(|v| v.ok())
                    .filter_map(|v| lua_value_to_json(&v).ok())
                    .collect();
                if !arr.is_empty() {
                    return Ok(serde_json::Value::Array(arr));
                }
            }

            // Fall back to object
            let mut map = serde_json::Map::new();
            for (k, v) in t.pairs::<Value, Value>().flatten() {
                let key = match k {
                    Value::String(s) => s
                        .clone()
                        .to_str()
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    Value::Integer(i) => i.to_string(),
                    _ => continue,
                };
                if let Ok(v) = lua_value_to_json(&v) {
                    map.insert(key, v);
                }
            }
            Ok(serde_json::Value::Object(map))
        }
        _ => Ok(serde_json::Value::Null),
    }
}

/// Convert serde_json::Value to a Lua Value.
fn json_to_lua_value(lua: &Lua, json: serde_json::Value) -> Result<Value, mlua::Error> {
    match json {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else if let Some(f) = n.as_f64() {
                Ok(Value::Number(f))
            } else {
                Ok(Value::Nil)
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(&s)?)),
        serde_json::Value::Array(arr) => {
            let t = lua.create_table()?;
            for (i, v) in arr.into_iter().enumerate() {
                t.set(i + 1, json_to_lua_value(lua, v)?)?;
            }
            Ok(Value::Table(t))
        }
        serde_json::Value::Object(map) => {
            let t = lua.create_table()?;
            for (k, v) in map {
                t.set(k, json_to_lua_value(lua, v)?)?;
            }
            Ok(Value::Table(t))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use luft_core::contract::backend::{
        AgentBackend, AgentCapabilities, AgentResult, BackendError, LogRef,
    };
    use luft_core::contract::finding::{Location, Severity};
    use luft_core::contract::ids::TokenUsage;
    use luft_core::scheduler::{BackendRegistry, RetryPolicy, SchedulerConfig};
    use luft_core::{AgentTask, MockBackend, MockBehavior};
    use crate::sdk::ReportSink;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::broadcast;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    // ── Helper: create a sample Finding ──────────────────────────

    fn sample_finding(title: &str) -> Finding {
        Finding {
            kind: "test".into(),
            severity: Severity::Info,
            title: title.into(),
            detail: "A detailed description".into(),
            location: Some(Location {
                file: "src/main.rs".into(),
                line: Some(42),
            }),
            evidence: vec!["line 42: suspect code".into()],
            data: serde_json::json!({"extra": "info"}),
        }
    }

    fn default_finding() -> Finding {
        sample_finding("Test Finding")
    }

    // ── Helper: create a quick scheduler for converge tests ──────

    fn converge_scheduler(backend: Arc<dyn AgentBackend>) -> Arc<Scheduler> {
        let config = SchedulerConfig {
            max_concurrency: 4,
            quota_per_run: 1000,
            retry: RetryPolicy::default(),
        };
        let registry = BackendRegistry::new().with(backend);
        Scheduler::new(config, registry, None)
    }

    fn test_run_ctx(scheduler: &Arc<Scheduler>) -> (RunContext, broadcast::Receiver<AgentEvent>) {
        let run_id = Uuid::now_v7();
        let rx = scheduler.init_run(run_id, 64);
        let (tx, _rx2) = broadcast::channel(64);
        let ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        (ctx, rx)
    }

    // ═══════════════════════════════════════════════════════════════
    // ConvergeConfig
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_default_config() {
        let c = ConvergeConfig::default();
        assert!(c.adversarial);
        assert!((c.vote_threshold - 0.7).abs() < f32::EPSILON);
        assert_eq!(c.max_rounds, 3);
        assert_eq!(c.producers_per_item, 1);
        assert_eq!(c.adversaries_per_finding, 1);
        assert!(c.model.is_none());
    }

    #[test]
    fn test_converge_config_debug_clone_serialize() {
        let c = ConvergeConfig::default();
        let _ = format!("{:?}", c);
        let _ = c.clone();
        let json = serde_json::to_string(&c).unwrap();
        let back: ConvergeConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(c.max_rounds, back.max_rounds);
    }

    // ═══════════════════════════════════════════════════════════════
    // parse_converge_options
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_parse_options_defaults() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        let cfg = parse_converge_options(&t);
        assert!(cfg.adversarial);
        assert!((cfg.vote_threshold - 0.7).abs() < f32::EPSILON);
        assert_eq!(cfg.max_rounds, 3);
        assert_eq!(cfg.producers_per_item, 1);
        assert_eq!(cfg.adversaries_per_finding, 1);
        assert!(cfg.model.is_none());
    }

    #[test]
    fn test_parse_options_all_fields() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.set("adversarial", "false").unwrap();
        t.set("vote_threshold", 0.5).unwrap();
        t.set("max_rounds", 5u32).unwrap();
        t.set("producers", 2u32).unwrap();
        t.set("adversaries", 3u32).unwrap();
        t.set("model", "gpt-4").unwrap();
        let cfg = parse_converge_options(&t);
        assert!(!cfg.adversarial);
        assert!((cfg.vote_threshold - 0.5).abs() < f32::EPSILON);
        assert_eq!(cfg.max_rounds, 5);
        assert_eq!(cfg.producers_per_item, 2);
        assert_eq!(cfg.adversaries_per_finding, 3);
        assert_eq!(cfg.model.as_deref(), Some("gpt-4"));
    }

    #[test]
    fn test_parse_options_adversarial_true_string() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.set("adversarial", "true").unwrap();
        assert!(parse_converge_options(&t).adversarial);
    }

    #[test]
    fn test_parse_options_empty_model() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.set("model", "").unwrap();
        assert!(parse_converge_options(&t).model.is_none());
    }

    #[test]
    fn test_parse_options_model_some() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.set("model", "claude").unwrap();
        assert_eq!(parse_converge_options(&t).model.as_deref(), Some("claude"));
    }

    // ═══════════════════════════════════════════════════════════════
    // extract_string
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_extract_string_valid() {
        let lua = Lua::new();
        assert_eq!(
            extract_string(lua.create_string("hello").unwrap()),
            Some("hello".into())
        );
    }

    #[test]
    fn test_extract_string_empty() {
        let lua = Lua::new();
        assert_eq!(
            extract_string(lua.create_string("").unwrap()),
            Some("".into())
        );
    }

    #[test]
    fn test_extract_string_invalid_utf8() {
        let lua = Lua::new();
        let s = lua.create_string([0xFF, 0xFE, 0x00]).unwrap();
        assert_eq!(extract_string(s), None);
    }

    // ═══════════════════════════════════════════════════════════════
    // lua_value_to_json — every match arm
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_lua_value_to_json_nil() {
        assert_eq!(
            lua_value_to_json(&Value::Nil).unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn test_lua_value_to_json_boolean() {
        assert_eq!(
            lua_value_to_json(&Value::Boolean(true)).unwrap(),
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            lua_value_to_json(&Value::Boolean(false)).unwrap(),
            serde_json::Value::Bool(false)
        );
    }

    #[test]
    fn test_lua_value_to_json_integer() {
        assert_eq!(
            lua_value_to_json(&Value::Integer(42)).unwrap(),
            serde_json::json!(42)
        );
        assert_eq!(
            lua_value_to_json(&Value::Integer(-5)).unwrap(),
            serde_json::json!(-5)
        );
    }

    #[test]
    fn test_lua_value_to_json_number() {
        assert_eq!(
            lua_value_to_json(&Value::Number(std::f64::consts::PI)).unwrap(),
            serde_json::json!(std::f64::consts::PI)
        );
    }

    #[test]
    fn test_lua_value_to_json_number_nan() {
        assert_eq!(
            lua_value_to_json(&Value::Number(f64::NAN)).unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn test_lua_value_to_json_number_infinity() {
        assert_eq!(
            lua_value_to_json(&Value::Number(f64::INFINITY)).unwrap(),
            serde_json::Value::Null
        );
        assert_eq!(
            lua_value_to_json(&Value::Number(f64::NEG_INFINITY)).unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn test_lua_value_to_json_string_valid() {
        let lua = Lua::new();
        let s = lua.create_string("hello world").unwrap();
        assert_eq!(
            lua_value_to_json(&Value::String(s)).unwrap(),
            serde_json::json!("hello world")
        );
    }

    #[test]
    fn test_lua_value_to_json_string_invalid_utf8() {
        let lua = Lua::new();
        let s = lua.create_string([0xFF, 0xFE]).unwrap();
        assert_eq!(
            lua_value_to_json(&Value::String(s)).unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn test_lua_value_to_json_table_as_array() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.set(1, "a").unwrap();
        t.set(2, "b").unwrap();
        t.set(3, "c").unwrap();
        assert_eq!(
            lua_value_to_json(&Value::Table(t)).unwrap(),
            serde_json::json!(["a", "b", "c"])
        );
    }

    #[test]
    fn test_lua_value_to_json_table_as_object() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.set("name", "test").unwrap();
        t.set("count", 42).unwrap();
        let val = lua_value_to_json(&Value::Table(t)).unwrap();
        let obj = val.as_object().unwrap();
        assert_eq!(obj["name"], "test");
        assert_eq!(obj["count"], 42);
    }

    #[test]
    fn test_lua_value_to_json_table_empty() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        assert_eq!(
            lua_value_to_json(&Value::Table(t)).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn test_lua_value_to_json_table_nested() {
        let lua = Lua::new();
        let inner = lua.create_table().unwrap();
        inner.set("key", "val").unwrap();
        let outer = lua.create_table().unwrap();
        outer.set("nested", inner).unwrap();
        let val = lua_value_to_json(&Value::Table(outer)).unwrap();
        assert_eq!(val, serde_json::json!({"nested": {"key": "val"}}));
    }

    #[test]
    fn test_lua_value_to_json_table_integer_key_in_object() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.set(42, "answer").unwrap();
        let val = lua_value_to_json(&Value::Table(t)).unwrap();
        assert_eq!(val, serde_json::json!({"42": "answer"}));
    }

    #[test]
    fn test_lua_value_to_json_function_catch_all() {
        let lua = Lua::new();
        let f = lua.create_function(|_, ()| Ok(())).unwrap();
        assert_eq!(
            lua_value_to_json(&Value::Function(f)).unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn test_lua_value_to_json_userdata_catch_all() {
        let lua = Lua::new();
        // Thread is a kind of userdata in Lua
        let thread = lua
            .create_thread(lua.load("return 1").into_function().unwrap())
            .unwrap();
        let val = Value::Thread(thread);
        assert_eq!(lua_value_to_json(&val).unwrap(), serde_json::Value::Null);
    }

    // ═══════════════════════════════════════════════════════════════
    // json_to_lua_value — every match arm
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_json_to_lua_value_null() {
        let lua = Lua::new();
        assert!(matches!(
            json_to_lua_value(&lua, serde_json::Value::Null).unwrap(),
            Value::Nil
        ));
    }

    #[test]
    fn test_json_to_lua_value_bool() {
        let lua = Lua::new();
        assert!(matches!(
            json_to_lua_value(&lua, serde_json::Value::Bool(true)).unwrap(),
            Value::Boolean(true)
        ));
    }

    #[test]
    fn test_json_to_lua_value_integer() {
        let lua = Lua::new();
        assert!(matches!(
            json_to_lua_value(&lua, serde_json::json!(42)).unwrap(),
            Value::Integer(42)
        ));
    }

    #[test]
    fn test_json_to_lua_value_float() {
        let lua = Lua::new();
        assert!(matches!(
            json_to_lua_value(&lua, serde_json::json!(std::f64::consts::PI)).unwrap(),
            Value::Number(n) if (n - std::f64::consts::PI).abs() < f64::EPSILON
        ));
    }

    #[test]
    fn test_json_to_lua_value_large_number() {
        let lua = Lua::new();
        // 1e200 fits in f64 but not i64 → should become a Lua number
        let val = json_to_lua_value(&lua, serde_json::json!(1e200)).unwrap();
        assert!(matches!(val, Value::Number(_)));
    }

    #[test]
    fn test_json_to_lua_value_string() {
        let lua = Lua::new();
        assert!(matches!(
            json_to_lua_value(&lua, serde_json::Value::String("hi".into())).unwrap(),
            Value::String(_)
        ));
    }

    #[test]
    fn test_json_to_lua_value_array() {
        let lua = Lua::new();
        let val = json_to_lua_value(&lua, serde_json::json!([1, 2, 3])).unwrap();
        assert!(matches!(val, Value::Table(_)));
    }

    #[test]
    fn test_json_to_lua_value_empty_array() {
        let lua = Lua::new();
        let val = json_to_lua_value(&lua, serde_json::json!([])).unwrap();
        assert!(matches!(val, Value::Table(_)));
    }

    #[test]
    fn test_json_to_lua_value_object() {
        let lua = Lua::new();
        let val = json_to_lua_value(&lua, serde_json::json!({"a": 1})).unwrap();
        assert!(matches!(val, Value::Table(_)));
    }

    #[test]
    fn test_json_to_lua_value_empty_object() {
        let lua = Lua::new();
        let val = json_to_lua_value(&lua, serde_json::json!({})).unwrap();
        assert!(matches!(val, Value::Table(_)));
    }

    #[test]
    fn test_json_to_lua_value_nested() {
        let lua = Lua::new();
        let val = json_to_lua_value(&lua, serde_json::json!({"a": {"b": [1, 2, 3]}})).unwrap();
        assert!(matches!(val, Value::Table(_)));
    }

    // ═══════════════════════════════════════════════════════════════
    // Struct construction & derive traits
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_converge_result_construction() {
        let r = ConvergeResult {
            surviving_items: vec![serde_json::json!({"k": "v"})],
            findings: vec![default_finding()],
            rounds: 2,
            converged: true,
            round_stats: vec![RoundStats {
                round: 1,
                items_input: 3,
                findings_generated: 5,
                findings_survived: 4,
                findings_refuted: 1,
                approval_rate: 0.8,
            }],
        };
        assert_eq!(r.rounds, 2);
        assert!(r.converged);
        assert_eq!(r.surviving_items.len(), 1);
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.round_stats.len(), 1);
    }

    #[test]
    fn test_converge_result_serialize_roundtrip() {
        let r = ConvergeResult {
            surviving_items: vec![],
            findings: vec![],
            rounds: 0,
            converged: true,
            round_stats: vec![],
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: ConvergeResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back.rounds, 0);
        assert!(back.converged);
    }

    #[test]
    fn test_converge_result_debug_clone() {
        let r = ConvergeResult {
            surviving_items: vec![],
            findings: vec![],
            rounds: 1,
            converged: false,
            round_stats: vec![],
        };
        let _ = format!("{:?}", r);
        let _ = r.clone();
    }

    #[test]
    fn test_round_stats_construction() {
        let s = RoundStats {
            round: 1,
            items_input: 10,
            findings_generated: 20,
            findings_survived: 15,
            findings_refuted: 5,
            approval_rate: 0.75,
        };
        assert_eq!(s.round, 1);
        assert_eq!(s.items_input, 10);
        assert_eq!(s.findings_generated, 20);
        assert_eq!(s.findings_survived, 15);
        assert_eq!(s.findings_refuted, 5);
        assert!((s.approval_rate - 0.75).abs() < f32::EPSILON);
    }

    #[test]
    fn test_round_stats_serialize_roundtrip() {
        let s = RoundStats {
            round: 2,
            items_input: 5,
            findings_generated: 8,
            findings_survived: 3,
            findings_refuted: 5,
            approval_rate: 0.4,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: RoundStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back.round, 2);
        assert!((back.approval_rate - 0.4).abs() < f32::EPSILON);
    }

    // ═══════════════════════════════════════════════════════════════
    // execute_convergence — empty items (early return)
    // ═══════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_execute_convergence_empty_items() {
        let scheduler = converge_scheduler(Arc::new(NoOpBackend));
        let (ctx, _rx) = test_run_ctx(&scheduler);

        let result = execute_convergence(
            vec![],
            "producer: {item}",
            "adversary: {finding}",
            ConvergeConfig::default(),
            &scheduler,
            &ctx,
        )
        .await
        .unwrap();

        assert!(result.surviving_items.is_empty());
        assert!(result.findings.is_empty());
        assert_eq!(result.rounds, 0);
        assert!(result.converged);
        assert!(result.round_stats.is_empty());
    }

    // ═══════════════════════════════════════════════════════════════
    // execute_convergence — items but backend returns no findings
    // ═══════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_execute_convergence_no_findings() {
        let mock = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::Value::Null,
                tokens: TokenUsage::default(),
                delay: Duration::from_millis(1),
            }],
        ));
        let scheduler = converge_scheduler(mock);
        let (ctx, _rx) = test_run_ctx(&scheduler);

        let result = execute_convergence(
            vec![serde_json::json!({"key": "val"})],
            "producer: {item}",
            "adversary: {finding}",
            ConvergeConfig::default(),
            &scheduler,
            &ctx,
        )
        .await
        .unwrap();

        // No findings generated → break before round stats pushed, so rounds=0.
        // Original items survive because state.items was never replaced.
        assert_eq!(result.rounds, 0);
        assert!(!result.converged);
        assert!(result.findings.is_empty());
        assert_eq!(
            result.surviving_items,
            vec![serde_json::json!({"key": "val"})]
        );
        assert!(result.round_stats.is_empty());
    }

    // ═══════════════════════════════════════════════════════════════
    // execute_convergence — findings survive through multiple rounds
    // ═══════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_execute_convergence_findings_survive() {
        let findings = vec![default_finding()];
        let backend = Arc::new(FindingsAlwaysBackend { findings });
        let scheduler = converge_scheduler(backend);
        let (ctx, _rx) = test_run_ctx(&scheduler);

        let result = execute_convergence(
            vec![serde_json::json!({"input": "item"})],
            "producer: {item}",
            "adversary: {finding}",
            ConvergeConfig::default(),
            &scheduler,
            &ctx,
        )
        .await
        .unwrap();

        // Round 1: finding generated, survives; Round 2: same finding → full
        // convergence detected because surviving == generated && round > 1.
        assert_eq!(result.rounds, 2);
        assert!(result.converged);
        assert!(!result.findings.is_empty());
        assert_eq!(result.round_stats.len(), 2);
        // Each round should have findings generated and survived
        for rs in &result.round_stats {
            assert_eq!(rs.findings_generated, 1);
            assert_eq!(rs.findings_survived, 1);
            assert_eq!(rs.findings_refuted, 0);
        }
    }

    // ═══════════════════════════════════════════════════════════════
    // execute_convergence — adversarial disabled
    // ═══════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_execute_convergence_non_adversarial() {
        let findings = vec![default_finding()];
        let backend = Arc::new(FindingsAlwaysBackend { findings });
        let scheduler = converge_scheduler(backend);
        let (ctx, _rx) = test_run_ctx(&scheduler);

        let config = ConvergeConfig {
            adversarial: false,
            ..ConvergeConfig::default()
        };

        let result = execute_convergence(
            vec![serde_json::json!({"x": 1})],
            "producer: {item}",
            "adversary: {finding}",
            config,
            &scheduler,
            &ctx,
        )
        .await
        .unwrap();

        // With adversarial=false, findings pass through unverified
        assert_eq!(result.rounds, 2);
        assert!(result.converged);
        assert!(!result.findings.is_empty());
    }

    // ═══════════════════════════════════════════════════════════════
    // execute_convergence — all findings refuted
    // ═══════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_execute_convergence_all_refuted() {
        let findings = vec![default_finding()];
        let backend = Arc::new(FindingsRefutingBackend { findings });
        let scheduler = converge_scheduler(backend);
        let (ctx, _rx) = test_run_ctx(&scheduler);

        let result = execute_convergence(
            vec![serde_json::json!({"x": 1})],
            "producer: {item}",
            "adversary: {finding}",
            ConvergeConfig::default(),
            &scheduler,
            &ctx,
        )
        .await
        .unwrap();

        // Round 1: finding generated, then refuted → empty surviving → converged
        assert_eq!(result.rounds, 1);
        assert!(result.converged);
        assert_eq!(result.round_stats.len(), 1);
        assert_eq!(result.round_stats[0].findings_generated, 1);
        assert_eq!(result.round_stats[0].findings_survived, 0);
        assert_eq!(result.round_stats[0].findings_refuted, 1);
    }

    // ═══════════════════════════════════════════════════════════════
    // execute_convergence with no adversarial (findings but adversarial disabled)
    // ═══════════════════════════════════════════════════════════════

    #[tokio::test]
    async fn test_execute_convergence_no_adversarial_findings_generated() {
        let findings = vec![default_finding()];
        let backend = Arc::new(FindingsAlwaysBackend { findings });
        let scheduler = converge_scheduler(backend);
        let (ctx, _rx) = test_run_ctx(&scheduler);

        let config = ConvergeConfig {
            adversarial: false,
            max_rounds: 1,
            ..ConvergeConfig::default()
        };

        let result = execute_convergence(
            vec![serde_json::json!({"x": 1})],
            "producer: {item}",
            "adversary: {finding}",
            config,
            &scheduler,
            &ctx,
        )
        .await
        .unwrap();

        assert_eq!(result.rounds, 1);
        // With max_rounds=1, we never hit the full convergence check (round>1)
        assert!(!result.converged);
        assert_eq!(result.findings.len(), 1);
    }

    // ═══════════════════════════════════════════════════════════════
    // register_converge_sdk — empty items via Lua
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_register_converge_sdk_empty_items() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let lua = Lua::new();
        let scheduler = converge_scheduler(Arc::new(NoOpBackend));
        let run_id = Uuid::now_v7();
        let (tx, _rx2) = broadcast::channel(64);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let report_sink: ReportSink = Arc::new(std::sync::Mutex::new(None));
        let handle = rt.handle().clone();
        let cx = SdkContext::new(run_ctx, scheduler, report_sink, None, handle);

        register_converge_sdk(&lua, &cx).unwrap();

        let globals = lua.globals();
        let converge: mlua::Function = globals.get("converge").unwrap();

        let items = lua.create_table().unwrap();
        let options = lua.create_table().unwrap();
        let result: mlua::Table = converge.call((items, options)).unwrap();

        let converged: bool = result.get("converged").unwrap();
        assert!(converged);
        let rounds: u32 = result.get("rounds").unwrap();
        assert_eq!(rounds, 0);
        let surviving: mlua::Table = result.get("surviving").unwrap();
        assert_eq!(surviving.len().unwrap(), 0);
        let findings: mlua::Table = result.get("findings").unwrap();
        assert_eq!(findings.len().unwrap(), 0);
    }

    #[test]
    fn test_register_converge_sdk_with_items() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let lua = Lua::new();
        let backend = Arc::new(FindingsAlwaysBackend {
            findings: vec![default_finding()],
        });
        let scheduler = converge_scheduler(backend);
        let run_id = Uuid::now_v7();
        let _rx = scheduler.init_run(run_id, 64);
        let (tx, _rx2) = broadcast::channel(64);
        let run_ctx = RunContext {
            run_id,
            cancel: CancellationToken::new(),
            events: tx,
        };
        let report_sink: ReportSink = Arc::new(std::sync::Mutex::new(None));
        let handle = rt.handle().clone();
        let cx = SdkContext::new(run_ctx, scheduler, report_sink, None, handle);

        register_converge_sdk(&lua, &cx).unwrap();

        let globals = lua.globals();
        let converge: mlua::Function = globals.get("converge").unwrap();

        let items = lua.create_table().unwrap();
        items.set(1, "test item").unwrap();
        let options = lua.create_table().unwrap();
        let result: mlua::Table = converge.call((items, options)).unwrap();

        let converged: bool = result.get("converged").unwrap();
        assert!(converged);
        let rounds: u32 = result.get("rounds").unwrap();
        assert!(rounds > 0);
        let findings: mlua::Table = result.get("findings").unwrap();
        assert!(findings.len().unwrap() > 0);
    }

    // ═══════════════════════════════════════════════════════════════
    // ProducerStats / VoteStats
    // ═══════════════════════════════════════════════════════════════

    #[test]
    fn test_producer_stats() {
        let s = ProducerStats {
            items_processed: 5,
            agents_run: 10,
            findings_generated: 20,
        };
        assert_eq!(s.items_processed, 5);
        assert_eq!(s.agents_run, 10);
        assert_eq!(s.findings_generated, 20);
    }

    #[test]
    fn test_producer_stats_default() {
        let s = ProducerStats::default();
        assert_eq!(s.items_processed, 0);
        assert_eq!(s.agents_run, 0);
        assert_eq!(s.findings_generated, 0);
    }

    #[test]
    fn test_vote_stats() {
        let s = VoteStats { approval_rate: 0.5 };
        assert!((s.approval_rate - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn test_vote_stats_default() {
        let s = VoteStats::default();
        assert!((s.approval_rate - 0.0).abs() < f32::EPSILON);
    }

    // ═══════════════════════════════════════════════════════════════
    // Custom test backends
    // ═══════════════════════════════════════════════════════════════

    /// Backend that always returns empty findings with AgentStatus::Ok.
    /// Used for tests that don't care about scheduler output.
    struct NoOpBackend;

    #[async_trait]
    impl AgentBackend for NoOpBackend {
        fn id(&self) -> &'static str {
            "noop"
        }
        fn capabilities(&self) -> AgentCapabilities {
            AgentCapabilities::default()
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn run(
            &self,
            task: AgentTask,
            _ctx: RunContext,
        ) -> Result<AgentResult, BackendError> {
            Ok(AgentResult {
                agent_id: task.agent_id,
                status: AgentStatus::Ok,
                output: serde_json::Value::Null,
                findings: vec![],
                tokens_used: TokenUsage::default(),
                artifacts: vec![],
                logs: LogRef::default(),
            })
        }
    }

    /// Backend that always returns configured findings with AgentStatus::Ok.
    /// Both producer and adversary phases will see findings + Ok status.
    struct FindingsAlwaysBackend {
        findings: Vec<Finding>,
    }

    #[async_trait]
    impl AgentBackend for FindingsAlwaysBackend {
        fn id(&self) -> &'static str {
            "findings-always"
        }
        fn capabilities(&self) -> AgentCapabilities {
            AgentCapabilities::default()
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn run(
            &self,
            task: AgentTask,
            _ctx: RunContext,
        ) -> Result<AgentResult, BackendError> {
            Ok(AgentResult {
                agent_id: task.agent_id,
                status: AgentStatus::Ok,
                output: serde_json::Value::Null,
                findings: self.findings.clone(),
                tokens_used: TokenUsage::default(),
                artifacts: vec![],
                logs: LogRef::default(),
            })
        }
    }

    /// Backend that returns findings but adversaries always return Error,
    /// causing all findings to be refuted.
    struct FindingsRefutingBackend {
        findings: Vec<Finding>,
    }

    #[async_trait]
    impl AgentBackend for FindingsRefutingBackend {
        fn id(&self) -> &'static str {
            "findings-refute"
        }
        fn capabilities(&self) -> AgentCapabilities {
            AgentCapabilities::default()
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        async fn run(
            &self,
            task: AgentTask,
            _ctx: RunContext,
        ) -> Result<AgentResult, BackendError> {
            Ok(AgentResult {
                agent_id: task.agent_id,
                status: AgentStatus::Error,
                output: serde_json::Value::Null,
                findings: self.findings.clone(),
                tokens_used: TokenUsage::default(),
                artifacts: vec![],
                logs: LogRef::default(),
            })
        }
    }
}
