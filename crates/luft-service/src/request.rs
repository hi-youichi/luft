//! Service-layer request types.
//!
//! Each struct is `#[derive(Deserialize, JsonSchema)]` so rmcp's
//! `Parameters<T>` can auto-deserialize without a manual assembler.

use crate::error::ServiceError;
use rmcp::schemars;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExecuteWorkflowRequest {
    pub script: Option<String>,
    pub path: Option<String>,
    pub resume_from_id: Option<String>,
    pub args: Option<Value>,
    pub concurrency: Option<u64>,
    /// Override the daemon default backend for this run (e.g. "codex", "opencode").
    /// Must be a registered backend id. Ignored when resuming.
    pub backend: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListRunsRequest {
    pub limit: Option<u64>,
    pub cursor: Option<String>,
    pub status_filter: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetRunStatusRequest {
    pub run_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetRunEventsRequest {
    pub run_id: String,
    pub since_event_id: Option<String>,
    pub offset: Option<u64>,
    pub events_limit: Option<u64>,
    pub types: Option<Vec<String>>,
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CancelRunRequest {
    pub run_id: String,
}

pub const MIN_CONCURRENCY: u64 = 1;
pub const MAX_CONCURRENCY: u64 = 64;
pub const DEFAULT_LIST_RUNS_LIMIT: u64 = 20;
pub const MAX_LIST_RUNS_LIMIT: u64 = 100;
pub const DEFAULT_EVENTS_LIMIT: u64 = 50;
pub const MAX_EVENTS_LIMIT: u64 = 500;
pub const STATUS_FILTERS: &[&str] = &["completed", "failed", "cancelled"];

impl ExecuteWorkflowRequest {
    pub fn validate(&self) -> Result<(), ServiceError> {
        let resume_from_id = self.resume_from_id.as_deref().filter(|s| !s.is_empty());
        let has_script = self.script.as_deref().is_some_and(|s| !s.trim().is_empty());
        let has_path = self.path.as_deref().is_some_and(|s| !s.is_empty());

        if resume_from_id.is_some() && (has_script || has_path) {
            return Err(ServiceError::InvalidParam(
                "'resume_from_id' is mutually exclusive with 'script' and 'path'".into(),
            ));
        }

        if let Some(c) = self.concurrency {
            if !(MIN_CONCURRENCY..=MAX_CONCURRENCY).contains(&c) {
                return Err(ServiceError::InvalidParam(format!(
                    "'concurrency' must be between {MIN_CONCURRENCY} and {MAX_CONCURRENCY}, got {c}"
                )));
            }
        }

        if let Some(ref b) = self.backend {
            if b.trim().is_empty() {
                return Err(ServiceError::InvalidParam(
                    "'backend' must be non-empty".into(),
                ));
            }
        }

        Ok(())
    }
}

impl ListRunsRequest {
    pub fn limit_or_default(&self) -> Result<u64, ServiceError> {
        match self.limit {
            None => Ok(DEFAULT_LIST_RUNS_LIMIT),
            Some(v) => {
                if !(1..=MAX_LIST_RUNS_LIMIT).contains(&v) {
                    return Err(ServiceError::InvalidParam(format!(
                        "'limit' must be between 1 and {MAX_LIST_RUNS_LIMIT}, got {v}"
                    )));
                }
                Ok(v)
            }
        }
    }

    pub fn status_filter_normalized(&self) -> Result<Option<String>, ServiceError> {
        match &self.status_filter {
            None => Ok(None),
            Some(s) => {
                let lower = s.to_lowercase();
                if !STATUS_FILTERS.contains(&lower.as_str()) {
                    return Err(ServiceError::InvalidParam(format!(
                        "'status_filter' must be one of completed/failed/cancelled, got {s}"
                    )));
                }
                Ok(Some(lower))
            }
        }
    }

    pub fn cursor_str(&self) -> Option<&str> {
        self.cursor.as_deref().filter(|s| !s.is_empty())
    }
}

impl GetRunEventsRequest {
    pub fn offset_or_default(&self) -> u64 {
        self.offset.unwrap_or(0)
    }

    pub fn events_limit_or_default(&self) -> u64 {
        self.events_limit
            .unwrap_or(DEFAULT_EVENTS_LIMIT)
            .clamp(1, MAX_EVENTS_LIMIT)
    }

    pub fn types_filter(&self) -> Option<Vec<String>> {
        self.types
            .as_ref()
            .filter(|t| !t.is_empty())
            .cloned()
    }
}
