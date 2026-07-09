//! # maestro-runtime
//!
//! **Sandboxed Lua orchestration VM.**
//!
//! The runtime executes Maestro orchestration scripts in a sandboxed
//! [`mlua`] VM. Scripts call SDK primitives that bridge to the scheduler
//! — the script is pure orchestration and cannot touch the filesystem, shell,
//! or network directly.
//!
//! ## SDK Primitives
//!
//! Lua scripts call these global functions:
//!
//! | Primitive | Description |
//! |-----------|-------------|
//! | `agent(opts)` | Run a single agent task; returns `AgentResult` |
//! | `parallel(items, map_fn)` | Fan-out: run `map_fn(item)` for each item concurrently, barrier-sync results |
//! | `pipeline(items, stages)` | Streaming pipeline: items flow through stages without barrier sync |
//! | `workflow(path, args?)` | Invoke a nested sub-workflow |
//! | `converge(opts)` | Multi-round consensus: agents iterate until convergence or round limit |
//! | `phase_begin(name)` / `phase_end(span)` | Structural progress spans for observability |
//! | `report(value)` | Emit the final workflow output (required) |
//! | `log(msg, level?)` | Structured log event |
//! | `budget(time_ms?, rounds?)` | Set runtime limits hint |
//! | `json.encode(value)` / `json.decode(str)` | JSON helpers (pure Lua, no `io`) |
//!
//! ## Sandbox Security Model
//!
//! The following Lua standard libraries are **removed** from the VM:
//!
//! - `io.*` — no file I/O
//! - `os.*` — no process / environment access
//! - `package.*` — no module loading
//! - `require`, `dofile`, `loadfile` — no external code execution
//!
//! Only orchestration primitives and `math`, `string`, `table`, `json` are available.
//! This guarantees that scripts — which may be **LLM-generated** — cannot escape
//! the orchestration layer.
//!
//! ## Validation
//!
//! Before execution, scripts are validated via [`validate`] (syntax + forbidden
//! globals) and [`validate_workflow`] (structural checks: `report()` presence,
//! span pairing, meta consistency).

mod converge;
mod error;
mod pipeline;
mod sandbox;
mod sdk;

pub use converge::{ConvergeConfig, ConvergeResult, RoundStats};
pub use error::{ExecLimits, ScriptError};
pub use pipeline::{
    PipelineConfig, PipelineError, PipelineExecutor, PipelineItem, PipelineItemResult,
    PipelineResult, PipelineStage, PipelineStats, StageResult, StageStatus,
};
pub use sandbox::{validate_script, validate_workflow, Runtime, WorkflowMeta, WorkflowValidation};

/// Validate a script without executing it (syntax + forbidden globals).
pub fn validate(script: &str) -> Result<(), ScriptError> {
    validate_script(script)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_syntax() {
        assert!(validate("return 1 + 2").is_ok());
    }

    #[test]
    fn validate_rejects_bad_syntax() {
        let err = validate("if true then").unwrap_err();
        assert!(matches!(err, ScriptError::Syntax(_)));
    }

    #[test]
    fn validate_empty_string() {
        assert!(validate("").is_ok());
    }

    #[test]
    fn validate_rejects_garbage() {
        let err = validate("~~ not lua ~~").unwrap_err();
        assert!(matches!(err, ScriptError::Syntax(_)));
    }

    #[test]
    fn validate_accepts_function_call() {
        assert!(validate("print('hello')").is_ok());
    }

    #[test]
    fn validate_rejects_mismatched_brackets() {
        let err = validate("local x = {1, 2, 3").unwrap_err();
        assert!(matches!(err, ScriptError::Syntax(_)));
    }

    #[test]
    fn validate_accepts_multi_line_script() {
        let script = r#"
            local x = 10
            local y = 20
            return x + y
        "#;
        assert!(validate(script).is_ok());
    }

    #[test]
    fn validate_rejects_operator_error() {
        let err = validate("local x = 1 +++ 2").unwrap_err();
        assert!(matches!(err, ScriptError::Syntax(_)));
    }

    #[test]
    fn validate_accepts_table_construction() {
        assert!(validate("local t = {a = 1, b = 2}").is_ok());
    }

    #[test]
    fn validate_accepts_function_def() {
        assert!(validate("function add(a, b) return a + b end").is_ok());
    }
}
