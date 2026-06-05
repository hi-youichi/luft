//! Structured finding schema (§1.3) — the data-plane output contract.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A structured finding reported by an agent via MCP `report_finding`.
/// The schema *is* the contract — agents emit these instead of free text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Category, e.g. "missing_auth" / "source".
    pub kind: String,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    /// Optional file:line locator.
    pub location: Option<Location>,
    /// Supporting evidence / citations.
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Free-form structured extension.
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub file: PathBuf,
    pub line: Option<u32>,
}
