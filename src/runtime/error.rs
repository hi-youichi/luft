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