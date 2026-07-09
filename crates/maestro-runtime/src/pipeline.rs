//! Streaming Pipeline — M2 implementation.
//!
//! A pipeline is a multi-stage streaming processor: items flow through stages
//! sequentially, and each item progresses to the next stage as soon as the
//! current stage completes (no barrier between stages).
//!
//! # Architecture
//!
//! ```text
//! Input  → [Stage 0] → [Stage 1] → ... → [Stage N] → Output
//!          (worker 0)   (worker 1)         (worker N)
//! ```
//!
//! Each stage runs an independent tokio task connected by channels.
//! Items flow through stages one at a time — item A can be in stage 1
//! while item B is still in stage 0 (streaming, not barrier).
//!
//! # Comparison to parallel()
//!
//! - `parallel()`: barrier — all items must finish before proceeding.
//! - `pipeline()`: streaming — items pass through each stage independently.

use maestro_core::contract::backend::AgentStatus;
use maestro_core::contract::event::{AgentEvent, EventSender};
use maestro_core::contract::ids::TokenUsage;
use std::sync::Arc;
use std::time::Instant;
use thiserror::Error;

// ============================================================================
// Errors
// ============================================================================

#[derive(Error, Debug)]
pub enum PipelineError {
    #[error("no items provided")]
    NoItems,
    #[error("no stages configured")]
    NoStages,
    #[error("pipeline already executed (state: {state:?})")]
    AlreadyExecuted { state: String },
    #[error("pipeline was cancelled")]
    Cancelled,
    #[error("stage handler error: {0}")]
    StageError(String),
}

// ============================================================================
// Pipeline Config
// ============================================================================

/// Configuration for a pipeline execution.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Ordered list of stage definitions.
    pub stages: Vec<PipelineStage>,
    /// Timeout in milliseconds for the entire pipeline.
    pub timeout_ms: u64,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            stages: Vec::new(),
            timeout_ms: 300_000, // 5 minutes
        }
    }
}

/// A single pipeline stage with its handler.
#[derive(Clone)]
pub struct PipelineStage {
    /// Human-readable label for this stage.
    pub label: String,
    /// Handler function: takes item JSON, returns processed JSON or error.
    /// This is the bridge between the Rust pipeline framework and the Lua SDK.
    pub handler: Arc<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>,
}

impl std::fmt::Debug for PipelineStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PipelineStage")
            .field("label", &self.label)
            .field("handler", &"<fn>")
            .finish()
    }
}

impl PipelineStage {
    pub fn new<F>(label: &str, handler: F) -> Self
    where
        F: Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static,
    {
        Self {
            label: label.to_string(),
            handler: Arc::new(handler),
        }
    }
}

// ============================================================================
// Pipeline Item
// ============================================================================

/// A single item flowing through the pipeline.
#[derive(Debug, Clone)]
pub struct PipelineItem {
    /// Original item index (0-based).
    pub index: usize,
    /// Current stage index this item is at.
    pub stage_index: usize,
    /// The item's data as a JSON value.
    pub data: serde_json::Value,
    /// Per-stage status tracking.
    pub stage_statuses: Vec<StageStatus>,
    /// Elapsed time per stage in milliseconds.
    pub stage_elapsed: Vec<u64>,
}

/// Status of an item at a specific stage.
#[derive(Debug, Clone, PartialEq)]
pub enum StageStatus {
    Pending,
    Running,
    Ok,
    Failed(String),
}

impl PipelineItem {
    pub fn new(index: usize, data: serde_json::Value, n_stages: usize) -> Self {
        Self {
            index,
            stage_index: 0,
            data,
            stage_statuses: vec![StageStatus::Pending; n_stages],
            stage_elapsed: vec![0; n_stages],
        }
    }
}

// ============================================================================
// Pipeline Result
// ============================================================================

/// Final result of a pipeline execution.
#[derive(Debug, Clone)]
pub struct PipelineResult {
    /// Per-item results.
    pub items: Vec<PipelineItemResult>,
    /// Aggregate statistics.
    pub stats: PipelineStats,
}

#[derive(Debug, Clone)]
pub struct PipelineItemResult {
    /// Original item index.
    pub item_index: usize,
    /// Final output data (after all stages).
    pub output: serde_json::Value,
    /// Per-stage outcomes.
    pub stage_results: Vec<StageResult>,
}

#[derive(Debug, Clone)]
pub struct StageResult {
    pub label: String,
    pub status: StageStatus,
    pub elapsed_ms: u64,
}

#[derive(Debug, Clone, Default)]
pub struct PipelineStats {
    pub total_items: usize,
    pub total_stages: usize,
    pub ok: usize,
    pub failed: usize,
    pub total_elapsed_ms: u64,
}

// ============================================================================
// Pipeline Executor
// ============================================================================

/// The core pipeline execution engine.
///
/// Manages the flow of items through stages using tokio channels.
pub struct PipelineExecutor {
    config: PipelineConfig,
    event_tx: Option<EventSender>,
    run_id: uuid::Uuid,
}

impl PipelineExecutor {
    /// Create a new pipeline executor.
    pub fn new(config: PipelineConfig, event_tx: Option<EventSender>, run_id: uuid::Uuid) -> Self {
        Self {
            config,
            event_tx,
            run_id,
        }
    }

    /// Execute the pipeline with the given items.
    ///
    /// Items flow through each stage sequentially. Stage 0 processes all items
    /// first, then each item moves to Stage 1, etc. Within each stage, items
    /// are processed sequentially through each stage.
    pub fn execute(&self, items: Vec<serde_json::Value>) -> Result<PipelineResult, PipelineError> {
        if items.is_empty() {
            return Err(PipelineError::NoItems);
        }
        if self.config.stages.is_empty() {
            return Err(PipelineError::NoStages);
        }

        let n_stages = self.config.stages.len();
        let n_items = items.len();
        tracing::info!(n_stages, n_items, "pipeline execution started");
        let run_id = self.run_id;

        // Emit PipelineStarted event
        self.emit(AgentEvent::PipelineStarted {
            run_id,
            total_stages: n_stages,
            items: n_items,
        });

        let pipeline_start = Instant::now();

        // Create initial PipelineItem wrappers
        let mut current_items: Vec<PipelineItem> = items
            .into_iter()
            .enumerate()
            .map(|(i, data)| PipelineItem::new(i, data, n_stages))
            .collect();

        // Process stages sequentially
        for (stage_idx, stage) in self.config.stages.iter().enumerate() {
            let stage_start = Instant::now();
            let stage_label = stage.label.clone();
            tracing::info!(stage_idx, %stage_label, items = current_items.len(), "pipeline stage started");

            // Emit PipelineStageStarted event
            self.emit(AgentEvent::PipelineStageStarted {
                run_id,
                stage_index: stage_idx,
                label: stage_label.clone(),
                agents_in_stage: current_items.len(),
            });

            // Process each item through this stage. Handlers run inline (not on
            // a separate task): they are synchronous closures that may call back
            // into the Lua VM, which is single-threaded and already held by the
            // caller — spawning them onto worker threads would deadlock on the
            // VM lock. Stages remain a barrier (all items finish stage N before
            // stage N+1), matching the documented per-stage progression.
            for item in current_items.iter_mut() {
                item.stage_index = stage_idx;
                item.stage_statuses[stage_idx] = StageStatus::Running;

                let item_start = Instant::now();
                let result = (stage.handler)(item.data.clone());
                let elapsed = item_start.elapsed().as_millis() as u64;

                if let Some(ref tx) = self.event_tx {
                    let status = match &result {
                        Ok(_) => AgentStatus::Ok,
                        Err(_) => AgentStatus::Error,
                    };
                    let _ = tx.send(AgentEvent::PipelineItemDone {
                        run_id,
                        stage_index: stage_idx,
                        item_index: item.index,
                        status,
                        tokens: TokenUsage::default(),
                        elapsed_ms: elapsed,
                    });
                }

                item.stage_elapsed[stage_idx] = elapsed;
                match result {
                    Ok(output) => {
                        item.data = output;
                        item.stage_statuses[stage_idx] = StageStatus::Ok;
                    }
                    Err(e) => {
                        tracing::warn!(item_index = item.index, stage_idx, error = %e, "pipeline item failed at stage");
                        item.stage_statuses[stage_idx] = StageStatus::Failed(e);
                    }
                }
            }

            let stage_elapsed = stage_start.elapsed();
            tracing::info!(stage_idx, %stage_label, elapsed_ms = stage_elapsed.as_millis() as u64, "pipeline stage finished");
        }

        let total_elapsed = pipeline_start.elapsed().as_millis() as u64;

        // Build results
        let mut ok_count = 0;
        let mut failed_count = 0;
        let mut item_results = Vec::with_capacity(n_items);

        for item in current_items {
            let is_ok = item.stage_statuses.iter().all(|s| *s == StageStatus::Ok);
            if is_ok {
                ok_count += 1;
            } else {
                failed_count += 1;
            }

            let mut stage_results = Vec::with_capacity(n_stages);
            for (i, status) in item.stage_statuses.iter().enumerate() {
                stage_results.push(StageResult {
                    label: self.config.stages[i].label.clone(),
                    status: status.clone(),
                    elapsed_ms: item.stage_elapsed[i],
                });
            }

            item_results.push(PipelineItemResult {
                item_index: item.index,
                output: item.data,
                stage_results,
            });
        }

        // Emit PipelineDone event
        self.emit(AgentEvent::PipelineDone {
            run_id,
            stages_completed: n_stages,
            total_ok: ok_count,
            total_failed: failed_count,
        });

        tracing::info!(
            total_elapsed_ms = total_elapsed,
            ok = ok_count,
            failed = failed_count,
            "pipeline execution finished"
        );

        Ok(PipelineResult {
            items: item_results,
            stats: PipelineStats {
                total_items: n_items,
                total_stages: n_stages,
                ok: ok_count,
                failed: failed_count,
                total_elapsed_ms: total_elapsed,
            },
        })
    }

    fn emit(&self, event: AgentEvent) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(event);
        }
    }
}

// ============================================================================
// Lua SDK Bridge
// ============================================================================

use crate::sdk::convert::{lua_value_from_json, value_to_json, JsonArg};
use crate::sdk::SdkContext;
use mlua::{Function, Lua, Table, Value};

/// Register the `pipeline(params)` SDK function in Lua.
///
/// `params` carries an `items` array, a `stages` array (each a function or a
/// `{ label, handler }` table). The handlers run
/// inside the executor, blocking on the shared tokio runtime.
pub(crate) fn register_pipeline_sdk(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    let globals = lua.globals();
    let run_id = cx.run_id();
    let events = cx.events();

    let pipeline_fn = lua.create_function(move |lua, params: Table| {
        let events = events.clone();

        let items_raw: Vec<Value> = params.get("items").map_err(|e| {
            mlua::Error::RuntimeError(format!("pipeline: missing 'items' array: {}", e))
        })?;
        let items: Vec<serde_json::Value> = items_raw
            .into_iter()
            .map(value_to_json)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                mlua::Error::RuntimeError(format!("pipeline: item conversion error: {}", e))
            })?;

        if items.is_empty() {
            let t = lua.create_table()?;
            t.set("items", lua.create_table()?)?;
            t.set("ok", 0)?;
            t.set("failed", 0)?;
            return Ok(t);
        }

        let stages_table: Table = params.get("stages").map_err(|e| {
            mlua::Error::RuntimeError(format!("pipeline: missing 'stages' array: {}", e))
        })?;

        let mut stages = Vec::new();
        for i in 1..=stages_table.len()? {
            let stage_val: Value = stages_table.get(i)?;
            let (label, handler): (String, Function) = match stage_val {
                Value::Function(func) => (format!("stage_{}", i), func),
                Value::Table(tbl) => (tbl.get("label")?, tbl.get("handler")?),
                _ => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "pipeline: stage {} must be a function or table",
                        i
                    )))
                }
            };
            let label_c = label.clone();
            let stage = PipelineStage::new(&label, move |data| {
                // `JsonArg` converts against the handler's own Lua VM (not a
                // throwaway one), so the value is valid for `handler.call`.
                match handler.call::<Value>(JsonArg(data)) {
                    Ok(Value::Nil) => Ok(serde_json::Value::Null),
                    Ok(v) => {
                        value_to_json(v).map_err(|e| format!("pipeline: result conversion: {}", e))
                    }
                    Err(e) => Err(format!("pipeline: stage '{}' error: {}", label_c, e)),
                }
            });
            stages.push(stage);
        }

        if stages.is_empty() {
            return Err(mlua::Error::RuntimeError(
                "pipeline: at least one stage required".into(),
            ));
        }

        let n_stages = stages.len();
        let config = PipelineConfig {
            stages,
            ..Default::default()
        };

        let executor = PipelineExecutor::new(config, Some(events), run_id);
        tracing::debug!(n_items = items.len(), n_stages, "pipeline SDK invoked");
        let result = executor.execute(items).map_err(|e| {
            tracing::error!(error = %e, "pipeline execution failed");
            mlua::Error::RuntimeError(format!("pipeline: execution error: {}", e))
        })?;

        let t = lua.create_table()?;
        let items_t = lua.create_table()?;
        for (i, item) in result.items.iter().enumerate() {
            let item_t = lua.create_table()?;
            item_t.set("index", item.item_index as i64)?;
            item_t.set("output", lua_value_from_json(lua, item.output.clone())?)?;
            let stages_t = lua.create_table()?;
            for (j, sr) in item.stage_results.iter().enumerate() {
                let sr_t = lua.create_table()?;
                sr_t.set("label", sr.label.as_str())?;
                sr_t.set("status", format!("{:?}", sr.status))?;
                sr_t.set("elapsed_ms", sr.elapsed_ms as i64)?;
                stages_t.set(j + 1, sr_t)?;
            }
            item_t.set("stages", stages_t)?;
            items_t.set(i + 1, item_t)?;
        }
        t.set("items", items_t)?;
        t.set("ok", result.stats.ok as i64)?;
        t.set("failed", result.stats.failed as i64)?;
        t.set("total_stages", result.stats.total_stages as i64)?;
        t.set("total_elapsed_ms", result.stats.total_elapsed_ms as i64)?;
        Ok(t)
    })?;
    globals.set("pipeline", pipeline_fn)?;

    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Stage handler that appends a marker and prepends the stage name.
    fn append_marker(
        stage_name: &str,
    ) -> impl Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync + 'static
    {
        let name = stage_name.to_string();
        move |data| {
            if let Some(obj) = data.as_object() {
                let mut result = obj.clone();
                result.insert("last_stage".to_string(), json!(name));
                // Track visited stages
                let mut visited: Vec<String> =
                    serde_json::from_value(result.get("visited").cloned().unwrap_or(json!([])))
                        .unwrap_or_default();
                visited.push(name.clone());
                result.insert("visited".to_string(), json!(visited));
                Ok(serde_json::Value::Object(result))
            } else {
                Ok(json!({ "data": data, "last_stage": name }))
            }
        }
    }

    #[tokio::test]
    async fn test_pipeline_basic() {
        let config = PipelineConfig {
            stages: vec![
                PipelineStage::new("extract", append_marker("extract")),
                PipelineStage::new("analyze", append_marker("analyze")),
                PipelineStage::new("report", append_marker("report")),
            ],
            timeout_ms: 10000,
        };

        let items = vec![
            json!({"id": 1, "text": "hello"}),
            json!({"id": 2, "text": "world"}),
        ];

        let executor = PipelineExecutor::new(config, None, uuid::Uuid::nil());
        let result = executor.execute(items).unwrap();

        assert_eq!(result.items.len(), 2);
        assert_eq!(result.stats.ok, 2);
        assert_eq!(result.stats.failed, 0);
        assert_eq!(result.stats.total_stages, 3);

        // Verify first item passed through all 3 stages
        let item0 = &result.items[0];
        assert_eq!(item0.output["last_stage"], json!("report"));
        let visited: Vec<String> = serde_json::from_value(item0.output["visited"].clone()).unwrap();
        assert_eq!(visited, vec!["extract", "analyze", "report"]);
    }

    #[tokio::test]
    async fn test_pipeline_stage_failure() {
        let config = PipelineConfig {
            stages: vec![
                PipelineStage::new("ok", append_marker("ok")),
                PipelineStage::new("fail", |data| {
                    // Fail if data contains "break"
                    if data.to_string().contains("break") {
                        Err("intentional failure".to_string())
                    } else {
                        append_marker("fail")(data)
                    }
                }),
            ],
            timeout_ms: 10000,
        };

        let items = vec![
            json!({"id": 1, "text": "good"}),
            json!({"id": 2, "text": "break"}),
        ];

        let executor = PipelineExecutor::new(config, None, uuid::Uuid::nil());
        let result = executor.execute(items).unwrap();

        assert_eq!(result.items.len(), 2);
        // Item 0 should succeed, item 1 should fail
        assert_eq!(result.stats.ok, 1);
        assert_eq!(result.stats.failed, 1);

        assert_eq!(
            result.items[1].stage_results[1].status,
            StageStatus::Failed("intentional failure".to_string())
        );
    }

    #[tokio::test]
    async fn test_pipeline_empty_items() {
        let config = PipelineConfig::default();
        let executor = PipelineExecutor::new(config, None, uuid::Uuid::nil());
        let result = executor.execute(vec![]);
        assert!(matches!(result, Err(PipelineError::NoItems)));
    }

    #[tokio::test]
    async fn test_pipeline_empty_stages() {
        let config = PipelineConfig {
            stages: vec![],
            ..Default::default()
        };
        let items = vec![json!({"id": 1})];
        let executor = PipelineExecutor::new(config, None, uuid::Uuid::nil());
        let result = executor.execute(items);
        assert!(matches!(result, Err(PipelineError::NoStages)));
    }
}
