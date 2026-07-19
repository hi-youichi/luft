//! # AgentBackend Contract
//!
//! The [`AgentBackend`] trait is the **control-plane boundary** between Luft's
//! orchestration runtime and the agent execution environment. Prompt goes in,
//! structured [`AgentResult`] comes out.
//!
//! ## Implementing a Backend
//!
//! ```no_run
//! use luft_core::contract::backend::*;
//! use async_trait::async_trait;
//!
//! struct MyBackend;
//!
//! impl MyBackend {
//!     fn new() -> Self { Self }
//! }
//!
//! #[async_trait]
//! impl AgentBackend for MyBackend {
//!     fn id(&self) -> &'static str { "my-backend" }
//!
//!     fn capabilities(&self) -> AgentCapabilities {
//!         AgentCapabilities {
//!             streaming: true,
//!             ..Default::default()
//!         }
//!     }
//!
//!     async fn run(&self, task: AgentTask, ctx: RunContext)
//!         -> Result<AgentResult, BackendError>
//!     {
//!         // 1. Observe cancellation
//!         if ctx.cancel.is_cancelled() {
//!             return Err(BackendError::Cancelled);
//!         }
//!
//!         // 2. Execute the agent task (your custom logic)
//!         let output = serde_json::json!({ "text": "hello" });
//!
//!         // 3. Return structured result
//!         Ok(AgentResult {
//!             agent_id: task.agent_id,
//!             status: AgentStatus::Ok,
//!             output,
//!             findings: vec![],
//!             tokens_used: Default::default(),
//!             artifacts: vec![],
//!             logs: LogRef::default(),
//!             thread_id: None,
//!         })
//!     }
//!
//!     fn as_any(&self) -> &dyn std::any::Any { self }
//! }
//! ```
use crate::contract::event::EventSender;
use crate::contract::finding::Finding;
use crate::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A pluggable agent backend (e.g. OpenCode via ACP). Prompt in, structured
/// result out.
///
/// # Contract
///
/// - **Cancellation**: implementations **must** observe `ctx.cancel` and return
///   promptly with [`BackendError::Cancelled`] when the token fires.
/// - **Id stability**: [`id()`](Self::id) must return a stable string for the
///   backend's lifetime — it is used as the registry key.
/// - **Thread safety**: the trait requires `Send + Sync`; backends are typically
///   wrapped in `Arc<dyn AgentBackend>` and shared across tasks.
/// - **Downcasting**: implement [`as_any()`](Self::as_any) by returning `self`
///   to allow callers to downcast to the concrete backend type.
///
/// See the [module docs](self) for a complete implementation example.
#[async_trait]
pub trait AgentBackend: Send + Sync {
    /// Stable backend id, e.g. "opencode".
    fn id(&self) -> &'static str;

    /// Capability declaration (v0.1: recorded/validated only; routing in v0.2).
    fn capabilities(&self) -> AgentCapabilities;

    /// Run one agent task to completion.
    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError>;

    /// Upcast hook for downcasting `&dyn AgentBackend` back to a concrete
    /// backend type. Standard Rust trait-object downcast pattern: each impl
    /// returns `self`, which `Any::downcast_ref` then narrows to `&Concrete`.
    /// No default impl is provided — `Self` is unsized on a trait object, so a
    /// default body `self` would not compile.
    fn as_any(&self) -> &dyn std::any::Any;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentCapabilities {
    pub streaming: bool,
    pub mcp_injection: bool,
    pub structured_output: bool,
    /// Known model ids; empty = unknown/any.
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTask {
    pub agent_id: AgentId,
    pub phase_id: PhaseId,
    pub prompt: String,
    pub model: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub agent_seq: u32,
    pub allowlist: Option<ToolPolicy>,
    pub workdir: PathBuf,
    /// Data-plane injection point (Luft MCP endpoint).
    pub mcp_endpoint: Option<McpEndpoint>,
    /// Idle timeout: maximum silence (no ACP notifications) before the backend
    /// kills the session. `None` = backend default (5 min).
    pub timeout: Option<Duration>,
    /// Optional JSON Schema (M4) for validating agent output.
    /// When set, the runtime validates the agent's output against this schema
    /// and may retry or reject if validation fails.
    pub output_schema: Option<serde_json::Value>,

    /// Per-agent working directory override from Lua `working_folder` opt.
    #[serde(default)]
    pub workdir_override: Option<PathBuf>,

    /// Thread ID for cross-process conversation resume (Loom SqliteSaver).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub agent_id: AgentId,
    pub status: AgentStatus,
    /// Structured output: prefers aggregated MCP findings, falls back to parsed
    /// final message.
    pub output: serde_json::Value,
    #[serde(default)]
    pub findings: Vec<Finding>,
    pub tokens_used: TokenUsage,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    pub logs: LogRef,

    /// Thread ID used during execution, echoed back for checkpoint linking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AgentStatus {
    Ok,
    Error,
    Cancelled,
    TimedOut,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Ok => "ok",
            AgentStatus::Error => "error",
            AgentStatus::Cancelled => "cancelled",
            AgentStatus::TimedOut => "timed_out",
        }
    }
}

/// Per-agent runtime context: cancellation + event sink + run association.
#[derive(Clone)]
pub struct RunContext {
    pub run_id: RunId,
    pub cancel: CancellationToken,
    pub events: EventSender,
}

/// Tool permission policy. v0.1 translates to a backend's acceptEdits + command
/// allowlist.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolPolicy {
    pub accept_edits: bool,
    pub allow_commands: Vec<String>,
    pub allow_mcp: Vec<String>,
    /// Explicit denies (highest precedence).
    pub deny: Vec<String>,
}

/// MCP data-plane endpoint injected into an agent for structured reporting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpEndpoint {
    /// Server name injected into the agent, e.g. "luft".
    pub name: String,
    pub url: String,
    pub run_id: RunId,
    pub agent_id: AgentId,
    pub auth_token: Option<String>,
}

#[derive(thiserror::Error, Debug)]
pub enum BackendError {
    #[error("spawn failed: {0}")]
    Spawn(String),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("connection error: {0}")]
    Connection(String),
    #[error("backend timed out")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("configuration error: {0}")]
    Config(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("execution error: {0}")]
    Execution(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl BackendError {
    /// Distinguish retryable (transient/timeout) from non-retryable (protocol/logic).
    pub fn is_retryable(&self) -> bool {
        matches!(self, BackendError::Timeout | BackendError::Spawn(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub key: String,
    pub path: Option<PathBuf>,
    pub inline: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LogRef {
    pub path: PathBuf,
}

#[cfg(test)]
mod tests {
    //! Tests for the `AgentStatus::as_str()` contract introduced by F5.
    //!
    //! The persisted `AgentResultCache.status` string is part of the on-disk
    //! checkpoint contract. Before F5 it was derived from `Debug` formatting,
    //! which silently broke when a variant was renamed (`TimedOut` → `"TimedOut"`
    //! → `"timedout"`). The explicit `as_str()` mapping pins the strings so that
    //! future renames cannot regress existing checkpoints.

    use super::*;

    #[test]
    fn as_str_ok_returns_ok() {
        assert_eq!(AgentStatus::Ok.as_str(), "ok");
    }

    #[test]
    fn as_str_error_returns_error() {
        assert_eq!(AgentStatus::Error.as_str(), "error");
    }

    #[test]
    fn as_str_cancelled_returns_cancelled() {
        assert_eq!(AgentStatus::Cancelled.as_str(), "cancelled");
    }

    #[test]
    fn as_str_timed_out_returns_snake_case_timed_out() {
        // The KEY F5 invariant: `TimedOut` Debug is "TimedOut" (lowercased
        // "timedout"), but the persisted string MUST be "timed_out" with an
        // underscore so it matches the surrounding snake_case contract.
        assert_eq!(AgentStatus::TimedOut.as_str(), "timed_out");
    }

    #[test]
    fn as_str_timed_out_differs_from_debug_lowercased() {
        // Regression guard: the bug being fixed. If this ever flips to
        // `format!("{:?}", status).to_lowercase()`, `TimedOut` would yield
        // "timedout" (no underscore) and silently corrupt existing checkpoints.
        let debug_lower = format!("{:?}", AgentStatus::TimedOut).to_lowercase();
        assert_ne!(AgentStatus::TimedOut.as_str(), debug_lower);
        assert_eq!(debug_lower, "timedout");
        assert_eq!(AgentStatus::TimedOut.as_str(), "timed_out");
    }

    #[test]
    fn as_str_values_are_unique() {
        let variants = [
            AgentStatus::Ok.as_str(),
            AgentStatus::Error.as_str(),
            AgentStatus::Cancelled.as_str(),
            AgentStatus::TimedOut.as_str(),
        ];
        for i in 0..variants.len() {
            for j in (i + 1)..variants.len() {
                assert_ne!(
                    variants[i], variants[j],
                    "AgentStatus::as_str() must produce distinct strings for each variant \
                     (collision between {:?} and {:?})",
                    variants[i], variants[j]
                );
            }
        }
    }

    #[test]
    fn as_str_values_are_non_empty_and_ascii() {
        for variant in [
            AgentStatus::Ok,
            AgentStatus::Error,
            AgentStatus::Cancelled,
            AgentStatus::TimedOut,
        ] {
            let s = variant.as_str();
            assert!(!s.is_empty(), "as_str() must not return empty strings");
            assert!(
                s.is_ascii(),
                "as_str() must return ASCII-only strings (got: {:?})",
                s
            );
        }
    }

    #[test]
    fn as_str_values_are_snake_case_or_lowercase() {
        // Each returned string must be either pure lowercase ASCII or
        // snake_case (lowercase ASCII letters separated by single underscores).
        // This matches the convention used elsewhere in the codebase
        // (CheckpointStatus via `rename_all = "lowercase"`, RunStatus, etc.).
        for variant in [
            AgentStatus::Ok,
            AgentStatus::Error,
            AgentStatus::Cancelled,
            AgentStatus::TimedOut,
        ] {
            let s = variant.as_str();
            for c in s.chars() {
                let ok = c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit();
                assert!(
                    ok,
                    "as_str() must return snake_case / lowercase (got {:?} in {:?})",
                    c, s
                );
            }
            assert!(
                !s.contains("__"),
                "as_str() must not produce consecutive underscores (got {:?})",
                s
            );
            assert!(
                !s.starts_with('_') && !s.ends_with('_'),
                "as_str() must not start or end with underscore (got {:?})",
                s
            );
        }
    }

    #[test]
    fn as_str_return_type_is_static_str() {
        // Compile-time check: the signature must return &'static str so the
        // string outlives any AgentStatus instance and the literal lives in
        // the binary's read-only data.
        fn returns_static(s: AgentStatus) -> &'static str {
            s.as_str()
        }
        let _: &'static str = returns_static(AgentStatus::Ok);
    }

    #[test]
    fn as_str_matches_storage_writer_canonical_mapping() {
        // The storage layer (`storage/writer.rs::agent_status_str`) already
        // canonicalises AgentStatus into the snake_case strings that the
        // on-disk SQLite tables consume. The F5 contract requires that
        // `AgentStatus::as_str()` agrees with this canonical mapping so
        // checkpoint persistence and storage persistence do not drift apart.
        const STORAGE_CANONICAL: &[(&str, AgentStatus)] = &[
            ("ok", AgentStatus::Ok),
            ("error", AgentStatus::Error),
            ("cancelled", AgentStatus::Cancelled),
            ("timed_out", AgentStatus::TimedOut),
        ];
        for (expected, variant) in STORAGE_CANONICAL {
            assert_eq!(
                variant.as_str(),
                *expected,
                "AgentStatus::{:?}::as_str() must equal {:?} (storage canonical)",
                variant,
                expected
            );
        }
    }
}
