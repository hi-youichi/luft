//! Build the final [`AgentResult`] from the accumulated message + stop reason.
//!
//! v0.1 uses the message-fallback path: structured findings are parsed out of
//! the agent's final text when present (MCP `report_finding` integration is P1).

use crate::core::contract::backend::{AgentResult, AgentStatus, AgentTask};
use crate::core::contract::finding::{Finding, Severity};
use crate::core::contract::ids::TokenUsage;

/// Assemble an [`AgentResult`] from a finished ACP run.
///
/// `stop_reason` is the `Debug` string of the ACP `StopReason` (matched loosely
/// so we don't depend on exact macro-generated variant names).
pub fn collect(
    task: &AgentTask,
    stop_reason: &str,
    message: String,
    tokens: TokenUsage,
) -> AgentResult {
    let status = status_from_stop_reason(stop_reason);
    let findings = extract_findings_from_output(&message);
    tracing::debug!(agent_id = %task.agent_id, ?status, findings = findings.len(), tokens = tokens.total(), stop_reason, "collecting agent result");
    let output = if !findings.is_empty() {
        serde_json::to_value(&findings).unwrap_or(serde_json::Value::Null)
    } else if let Ok(json) = serde_json::from_str::<serde_json::Value>(&message) {
        if json.is_object() {
            json
        } else {
            serde_json::json!({ "text": message })
        }
    } else if let Some(json) = extract_last_json_block(&message) {
        json
    } else {
        serde_json::json!({ "text": message })
    };

    AgentResult {
        agent_id: task.agent_id,
        status,
        output,
        findings,
        tokens_used: tokens,
        artifacts: vec![],
        logs: Default::default(),
    }
}

fn status_from_stop_reason(s: &str) -> AgentStatus {
    if s.contains("EndTurn") {
        AgentStatus::Ok
    } else if s.contains("Cancel") {
        AgentStatus::Cancelled
    } else {
        // MaxTokens / MaxTurns / Refused / unknown → treat as error.
        AgentStatus::Error
    }
}

/// Parse structured findings out of agent text (raw JSON or fenced code block).
pub fn extract_findings_from_output(output: &str) -> Vec<Finding> {
    let mut findings = Vec::new();

    let mut harvest = |json: &serde_json::Value| {
        if let Some(arr) = json.get("findings").and_then(|f| f.as_array()) {
            findings.extend(arr.iter().filter_map(parse_finding));
        } else if let Some(item) = json.get("finding") {
            findings.extend(parse_finding(item));
        }
    };

    if let Ok(json) = serde_json::from_str::<serde_json::Value>(output) {
        harvest(&json);
    }
    for block in output.split("```") {
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(block.trim()) {
            harvest(&json);
        }
    }

    findings
}

fn extract_last_json_block(message: &str) -> Option<serde_json::Value> {
    let mut last_json: Option<serde_json::Value> = None;
    for block in message.split("```") {
        let trimmed = block.trim();
        let candidate = trimmed
            .strip_prefix("json")
            .or_else(|| trimmed.strip_prefix("JSON"))
            .map(|s| s.trim())
            .unwrap_or(trimmed);
        if let Ok(json) = serde_json::from_str::<serde_json::Value>(candidate) {
            if json.is_object() {
                last_json = Some(json);
            }
        }
    }
    last_json
}

fn parse_finding(json: &serde_json::Value) -> Option<Finding> {
    let kind = json.get("kind")?.as_str()?.to_string();
    let severity = match json
        .get("severity")
        .and_then(|s| s.as_str())
        .unwrap_or("info")
        .to_lowercase()
        .as_str()
    {
        "critical" => Severity::Critical,
        "high" => Severity::High,
        "medium" => Severity::Medium,
        "low" => Severity::Low,
        _ => Severity::Info,
    };
    let str_field = |k: &str| json.get(k).and_then(|t| t.as_str()).unwrap_or("").to_string();
    let evidence = json
        .get("evidence")
        .and_then(|e| e.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    Some(Finding {
        kind,
        severity,
        title: str_field("title"),
        detail: str_field("detail"),
        location: None,
        evidence,
        data: serde_json::Value::Null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> AgentTask {
        AgentTask {
            agent_id: uuid::Uuid::now_v7(),
            phase_id: 0,
            prompt: String::new(),
            model: None,
            allowlist: None,
            workdir: std::path::PathBuf::from("."),
            mcp_endpoint: None,
            timeout: None,
            output_schema: None,
        }
    }

    #[test]
    fn end_turn_is_ok_text_output() {
        let r = collect(&task(), "EndTurn", "hello".into(), TokenUsage::default());
        assert_eq!(r.status, AgentStatus::Ok);
        assert_eq!(r.output, serde_json::json!({ "text": "hello" }));
    }

    #[test]
    fn cancelled_stop_reason_maps() {
        let r = collect(&task(), "Cancelled", String::new(), TokenUsage::default());
        assert_eq!(r.status, AgentStatus::Cancelled);
    }

    #[test]
    fn findings_become_output() {
        let msg = r#"{"findings":[{"kind":"bug","severity":"high","title":"X","detail":"Y"}]}"#;
        let r = collect(&task(), "EndTurn", msg.into(), TokenUsage::default());
        assert_eq!(r.findings[0].kind, "bug");
        assert!(r.output.is_array());
    }

    #[test]
    fn raw_json_object_becomes_output() {
        let msg = r#"{"files_deleted":["src/ws/","src/commands/serve.rs"]}"#;
        let r = collect(&task(), "EndTurn", msg.into(), TokenUsage::default());
        assert_eq!(r.output["files_deleted"][0], "src/ws/");
    }

    #[test]
    fn json_in_code_block_becomes_output() {
        let msg = "Done!\n```json\n{\"ok\":true,\"output\":\"all good\"}\n```\n";
        let r = collect(&task(), "EndTurn", msg.into(), TokenUsage::default());
        assert_eq!(r.output["ok"], true);
        assert_eq!(r.output["output"], "all good");
    }

    #[test]
    fn plain_text_still_wraps_as_text() {
        let msg = "I deleted the files as requested.";
        let r = collect(&task(), "EndTurn", msg.into(), TokenUsage::default());
        assert_eq!(r.output, serde_json::json!({"text": msg}));
    }
}
