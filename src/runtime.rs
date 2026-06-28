//! `runtime` — mlua orchestration runtime (M2). See code-design §4.
//!
//! The runtime executes Lua orchestration scripts in a sandboxed mlua VM.
//! Scripts call SDK primitives (`agent`, `parallel`, `converge`, `report`) which
//! bridge to the scheduler. The sandbox blocks `io`/`os`/`fs`/`network`.

mod converge;
mod error;
mod pipeline;
mod sandbox;
mod sdk;

pub use converge::{ConvergeConfig, ConvergeResult, RoundStats};
pub use error::{ExecLimits, ScriptError};
pub use pipeline::{PipelineConfig, PipelineError, PipelineExecutor, PipelineItem, PipelineItemResult, PipelineResult, PipelineStage, PipelineStats, StageResult, StageStatus};
pub use sandbox::{Runtime, WorkflowMeta, WorkflowValidation, validate_script, validate_workflow};


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