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

use crate::core::contract::backend::{AgentStatus, RunContext};
use crate::core::contract::event::AgentEvent;
use crate::core::contract::finding::Finding;
use crate::core::Scheduler;
use crate::runtime::error::ScriptError;
use crate::runtime::sdk::SdkContext;
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

    tracing::info!(n_items = items.len(), max_rounds = config.max_rounds, adversarial = config.adversarial, "converge started");

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
            tracing::debug!(round, n_findings = findings.len(), adversaries = config.adversaries_per_finding, "adversarial verification started");
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

    tracing::info!(rounds = state.round_stats.len(), converged = state.converged, surviving = state.items.len(), total_findings = state.findings.len(), "converge finished");
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
            let prompt = prompt_template.replace("{item}", &serde_json::to_string(item).unwrap_or_default());
            let agent_id = uuid::Uuid::now_v7();
            let task = crate::core::contract::backend::AgentTask {
                agent_id,
                phase_id: 2, // Converge phase
                prompt,
                model: model.clone(),
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

            let task = crate::core::contract::backend::AgentTask {
                agent_id,
                phase_id: 2,
                prompt,
                model: model.clone(),
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

    tracing::debug!(surviving = surviving.len(), refuted = findings.len() - surviving.len(), approval_rate, "adversarial voting completed");

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
        tracing::debug!(adversarial = config.adversarial, max_rounds = config.max_rounds, "converge SDK invoked");
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
                "Review this finding and determine if it is valid or should be refuted: {finding}".to_string()
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
                tracing::info!(rounds = res.rounds, converged = res.converged, surviving = res.surviving_items.len(), elapsed_ms, "converge SDK completed");
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
            for pair in t.pairs::<Value, Value>() {
                if let Ok((k, v)) = pair {
                    let key = match k {
                        Value::String(s) => s.clone().to_str().map(|s| s.to_string()).unwrap_or_default(),
                        Value::Integer(i) => i.to_string(),
                        _ => continue,
                    };
                    if let Ok(v) = lua_value_to_json(&v) {
                        map.insert(key, v);
                    }
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

    #[test]
    fn test_default_config() {
        let config = ConvergeConfig::default();
        assert!(config.adversarial);
        assert_eq!(config.vote_threshold, 0.7);
        assert_eq!(config.max_rounds, 3);
    }

    #[test]
    fn test_lua_value_conversion() {
        let lua = Lua::new();
        let json = serde_json::json!({
            "name": "test",
            "count": 42,
            "active": true
        });
        let value = json_to_lua_value(&lua, json).unwrap();
        assert!(matches!(value, Value::Table(_)));
    }
}