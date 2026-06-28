//! Script execution errors (code-design §4.7).

use thiserror::Error;

/// Execution limits for a Lua script (instruction count, wall clock, memory).
#[derive(Debug, Clone)]
pub struct ExecLimits {
    /// Maximum Lua VM instruction count (0 = unlimited).
    pub instruction_limit: u64,
    /// Maximum wall-clock time (None = unlimited).
    pub wall_clock_ms: Option<u64>,
    /// Maximum heap size in bytes (0 = unlimited).
    pub memory_limit_bytes: u64,
}

impl Default for ExecLimits {
    fn default() -> Self {
        Self {
            instruction_limit: 1_000_000,
            wall_clock_ms: Some(300_000), // 5 minutes
            memory_limit_bytes: 128 * 1024 * 1024, // 128 MB
        }
    }
}

/// Errors that can occur during script execution.
#[derive(Error, Debug)]
pub enum ScriptError {
    #[error("syntax error: {0}")]
    Syntax(String),

    #[error("sandbox violation: attempted to access forbidden global `{name}`")]
    SandboxViolation { name: String },

    #[error("instruction limit exceeded (limit: {0})")]
    InstructionLimitExceeded(u64),

    #[error("wall-clock timeout ({0}ms)")]
    WallClockTimeout(u64),

    #[error("memory limit exceeded ({0} bytes)")]
    MemoryLimitExceeded(u64),

    #[error("agent error: {0}")]
    AgentError(String),

    #[error("serialization error: {0}")]
    SerdeError(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("script is missing a `function main() ... end` entry point")]
    MissingMain,
}

impl From<mlua::Error> for ScriptError {
    fn from(e: mlua::Error) -> Self {
        use mlua::Error::*;
        match e {
            SyntaxError { message, .. } => ScriptError::Syntax(message),
            RuntimeError(msg) => {
                // Try to detect sandbox violations from error messages
                if msg.contains("forbidden") || msg.contains("not allowed") {
                    ScriptError::SandboxViolation { name: msg.clone() }
                } else if msg.contains("instruction limit") {
                    ScriptError::InstructionLimitExceeded(0)
                } else if msg.contains("timeout") {
                    ScriptError::WallClockTimeout(0)
                } else {
                    ScriptError::AgentError(msg)
                }
            }
            _ => ScriptError::Internal(e.to_string()),
        }
    }
}

impl From<serde_json::Error> for ScriptError {
    fn from(e: serde_json::Error) -> Self {
        ScriptError::SerdeError(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // ExecLimits
    // -----------------------------------------------------------------------
    #[test]
    fn exec_limits_default() {
        let limits = ExecLimits::default();
        assert_eq!(limits.instruction_limit, 1_000_000);
        assert_eq!(limits.wall_clock_ms, Some(300_000));
        assert_eq!(limits.memory_limit_bytes, 128 * 1024 * 1024);
    }

    #[test]
    fn exec_limits_debug_and_clone() {
        let a = ExecLimits::default();
        let b = a.clone();
        assert_eq!(a.instruction_limit, b.instruction_limit);
        let _ = format!("{:?}", a); // Debug
    }

    // -----------------------------------------------------------------------
    // ScriptError – Display formatting for every variant
    // -----------------------------------------------------------------------
    #[test]
    fn display_syntax() {
        let err = ScriptError::Syntax("unexpected symbol".into());
        assert_eq!(err.to_string(), "syntax error: unexpected symbol");
    }

    #[test]
    fn display_sandbox_violation() {
        let err = ScriptError::SandboxViolation { name: "os".into() };
        assert_eq!(
            err.to_string(),
            "sandbox violation: attempted to access forbidden global `os`"
        );
    }

    #[test]
    fn display_instruction_limit() {
        let err = ScriptError::InstructionLimitExceeded(50_000);
        assert_eq!(err.to_string(), "instruction limit exceeded (limit: 50000)");
    }

    #[test]
    fn display_wall_clock_timeout() {
        let err = ScriptError::WallClockTimeout(300_000);
        assert_eq!(err.to_string(), "wall-clock timeout (300000ms)");
    }

    #[test]
    fn display_memory_limit() {
        let err = ScriptError::MemoryLimitExceeded(134_217_728);
        assert_eq!(err.to_string(), "memory limit exceeded (134217728 bytes)");
    }

    #[test]
    fn display_agent_error() {
        let err = ScriptError::AgentError("something went wrong".into());
        assert_eq!(err.to_string(), "agent error: something went wrong");
    }

    #[test]
    fn display_serde_error() {
        let err = ScriptError::SerdeError("invalid type".into());
        assert_eq!(err.to_string(), "serialization error: invalid type");
    }

    #[test]
    fn display_internal() {
        let err = ScriptError::Internal("unexpected state".into());
        assert_eq!(err.to_string(), "internal error: unexpected state");
    }

    // -----------------------------------------------------------------------
    // ScriptError – Debug derives (smoke-check that all variants are Debug)
    // -----------------------------------------------------------------------
    #[test]
    fn debug_format() {
        let variants: Vec<ScriptError> = vec![
            ScriptError::Syntax("x".into()),
            ScriptError::SandboxViolation { name: "x".into() },
            ScriptError::InstructionLimitExceeded(1),
            ScriptError::WallClockTimeout(1),
            ScriptError::MemoryLimitExceeded(1),
            ScriptError::AgentError("x".into()),
            ScriptError::SerdeError("x".into()),
            ScriptError::Internal("x".into()),
        ];
        for v in &variants {
            let _ = format!("{:?}", v);
        }
    }

    // -----------------------------------------------------------------------
    // From<mlua::Error>
    // -----------------------------------------------------------------------
    #[test]
    fn from_mlua_syntax_error() {
        let e = mlua::Error::SyntaxError {
            message: "unexpected symbol near ')'".into(),
            incomplete_input: false,
        };
        match ScriptError::from(e) {
            ScriptError::Syntax(msg) => assert_eq!(msg, "unexpected symbol near ')'"),
            other => panic!("expected Syntax, got {other:?}"),
        }
    }

    #[test]
    fn from_mlua_runtime_forbidden() {
        let e = mlua::Error::RuntimeError("forbidden global 'os'".into());
        match ScriptError::from(e) {
            ScriptError::SandboxViolation { name } => {
                assert_eq!(name, "forbidden global 'os'");
            }
            other => panic!("expected SandboxViolation, got {other:?}"),
        }
    }

    #[test]
    fn from_mlua_runtime_not_allowed() {
        let e = mlua::Error::RuntimeError("this function is not allowed".into());
        match ScriptError::from(e) {
            ScriptError::SandboxViolation { name } => {
                assert_eq!(name, "this function is not allowed");
            }
            other => panic!("expected SandboxViolation, got {other:?}"),
        }
    }

    #[test]
    fn from_mlua_runtime_instruction_limit() {
        let e = mlua::Error::RuntimeError("instruction limit exceeded".into());
        match ScriptError::from(e) {
            ScriptError::InstructionLimitExceeded(limit) => assert_eq!(limit, 0),
            other => panic!("expected InstructionLimitExceeded, got {other:?}"),
        }
    }

    #[test]
    fn from_mlua_runtime_timeout() {
        let e = mlua::Error::RuntimeError("timeout reached".into());
        match ScriptError::from(e) {
            ScriptError::WallClockTimeout(ms) => assert_eq!(ms, 0),
            other => panic!("expected WallClockTimeout, got {other:?}"),
        }
    }

    #[test]
    fn from_mlua_runtime_generic_agent_error() {
        let e = mlua::Error::RuntimeError("some random Lua error".into());
        match ScriptError::from(e) {
            ScriptError::AgentError(msg) => assert_eq!(msg, "some random Lua error"),
            other => panic!("expected AgentError, got {other:?}"),
        }
    }

    #[test]
    fn from_mlua_non_runtime_internal() {
        // MemoryError is not SyntaxError or RuntimeError, so it hits the catch-all
        let e = mlua::Error::MemoryError("OOM".into());
        match ScriptError::from(e) {
            ScriptError::Internal(msg) => assert!(msg.contains("memory error")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn from_mlua_safety_error_internal() {
        let e = mlua::Error::SafetyError("unsafe operation".into());
        match ScriptError::from(e) {
            ScriptError::Internal(msg) => assert!(msg.contains("safety error")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // From<serde_json::Error>
    // -----------------------------------------------------------------------
    #[test]
    fn from_serde_json_error() {
        let invalid: serde_json::Error = serde_json::from_str::<()>("!").unwrap_err();
        match ScriptError::from(invalid) {
            ScriptError::SerdeError(msg) => assert!(!msg.is_empty()),
            other => panic!("expected SerdeError, got {other:?}"),
        }
    }
}