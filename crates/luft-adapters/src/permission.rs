//! Non-interactive permission decisions for ACP tool requests.
//!
//! v0.1 never blocks on a human: every `session/request_permission` is decided
//! synchronously from the task's [`ToolPolicy`]. The decision logic ([`decide`])
//! is pure and unit-tested; [`extract_inputs`] adapts an ACP request into the
//! pure inputs.

use luft_core::contract::backend::ToolPolicy;
use agent_client_protocol::schema::RequestPermissionRequest;

/// Outcome of an automatic permission decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Approve (the handler selects the first offered option).
    Approve,
    /// Deny with a human-readable reason.
    Deny(String),
}

/// Request facts extracted from an ACP permission request, decoupled from the
/// ACP schema so [`decide`] stays pure and testable.
#[derive(Debug, Clone, Default)]
pub struct PermissionInputs {
    /// Shell command being requested, if any.
    pub command: Option<String>,
    /// Whether this is a file edit/write request.
    pub is_file_edit: bool,
    /// MCP tool name, if this is an MCP tool request.
    pub mcp_tool: Option<String>,
}

/// Decide whether to approve a tool request given the task's policy.
///
/// With **no policy** we default to approve so a self-contained agent (opencode)
/// can do its work in v0.1. With a policy we honour, in order:
/// deny-list → `accept_edits` → `allow_commands` → `allow_mcp`.
pub fn decide(policy: Option<&ToolPolicy>, input: &PermissionInputs) -> Decision {
    if let Some(tool) = &input.mcp_tool {
        if tool == "structured_output" {
            return Decision::Approve;
        }
    }

    let policy = match policy {
        None => return Decision::Approve,
        Some(p) => p,
    };

    // Deny list wins.
    if let Some(cmd) = &input.command {
        if policy.deny.iter().any(|d| cmd.contains(d.as_str())) {
            tracing::warn!(%cmd, "permission denied: command matches deny list");
            return Decision::Deny(format!("command matches deny list: {cmd}"));
        }
    }

    if input.is_file_edit {
        return if policy.accept_edits {
            tracing::debug!("permission: file edit approved");
            Decision::Approve
        } else {
            tracing::warn!("permission denied: file edit not allowed");
            Decision::Deny("accept_edits=false".into())
        };
    }

    if let Some(cmd) = &input.command {
        return if policy
            .allow_commands
            .iter()
            .any(|p| cmd.starts_with(p.as_str()))
        {
            tracing::debug!(%cmd, "permission: command approved");
            Decision::Approve
        } else {
            tracing::warn!(%cmd, "permission denied: command not in allowlist");
            Decision::Deny(format!("command not in allowlist: {cmd}"))
        };
    }

    if let Some(tool) = &input.mcp_tool {
        return if policy.allow_mcp.iter().any(|n| n == tool) {
            tracing::debug!(%tool, "permission: MCP tool approved");
            Decision::Approve
        } else {
            tracing::warn!(%tool, "permission denied: MCP tool not allowed");
            Decision::Deny(format!("mcp tool not allowed: {tool}"))
        };
    }

    // Unknown request type with a policy present: opencode self-manages reads.
    Decision::Approve
}

/// Best-effort extraction of [`PermissionInputs`] from an ACP request.
///
/// The ACP request shape is macro-generated; rather than depend on exact nested
/// types we inspect the serialized JSON heuristically. Since the default policy
/// is `None` (→ approve), this only affects policy-constrained runs.
pub fn extract_inputs(req: &RequestPermissionRequest) -> PermissionInputs {
    let v = serde_json::to_value(req).unwrap_or(serde_json::Value::Null);
    parse_inputs_from_json(&v)
}

/// Extract [`PermissionInputs`] from an already-serialized JSON value.
///
/// Extracted for testing — the fallback to [`serde_json::Value::Null`] inside
/// [`extract_inputs`] is covered by calling this with `Null` directly.
fn parse_inputs_from_json(v: &serde_json::Value) -> PermissionInputs {
    let raw = v.to_string();
    let is_file_edit =
        find_str_field(v, "kind").as_deref() == Some("edit") || raw.contains("write_text_file");
    let command = find_str_field(v, "command");
    let mcp_tool = find_str_field(v, "tool")
        .or_else(|| find_str_field(v, "name"))
        .filter(|_n| raw.contains("mcp") || raw.contains("structured_output"));
    PermissionInputs {
        command,
        is_file_edit,
        mcp_tool,
    }
}

/// Find the first string value for `key` anywhere in a JSON tree.
fn find_str_field(v: &serde_json::Value, key: &str) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get(key) {
                return Some(s.clone());
            }
            map.values().find_map(|child| find_str_field(child, key))
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(|child| find_str_field(child, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(accept_edits: bool, allow: &[&str], deny: &[&str]) -> ToolPolicy {
        ToolPolicy {
            accept_edits,
            allow_commands: allow.iter().map(|s| s.to_string()).collect(),
            allow_mcp: vec![],
            deny: deny.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn no_policy_approves() {
        assert_eq!(
            decide(None, &PermissionInputs::default()),
            Decision::Approve
        );
    }

    #[test]
    fn deny_list_blocks_command() {
        let p = policy(true, &["rm"], &["rm -rf"]);
        let input = PermissionInputs {
            command: Some("rm -rf /".into()),
            ..Default::default()
        };
        assert!(matches!(decide(Some(&p), &input), Decision::Deny(_)));
    }

    #[test]
    fn accept_edits_gate() {
        let edit = PermissionInputs {
            is_file_edit: true,
            ..Default::default()
        };
        assert_eq!(
            decide(Some(&policy(true, &[], &[])), &edit),
            Decision::Approve
        );
        assert!(matches!(
            decide(Some(&policy(false, &[], &[])), &edit),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn allow_commands_prefix() {
        let p = policy(false, &["cargo"], &[]);
        let ok = PermissionInputs {
            command: Some("cargo test".into()),
            ..Default::default()
        };
        let bad = PermissionInputs {
            command: Some("curl evil".into()),
            ..Default::default()
        };
        assert_eq!(decide(Some(&p), &ok), Decision::Approve);
        assert!(matches!(decide(Some(&p), &bad), Decision::Deny(_)));
    }

    // --- structured_output early return (lines 38-41) ---
    #[test]
    fn structured_output_approved() {
        let input = PermissionInputs {
            mcp_tool: Some("structured_output".into()),
            ..Default::default()
        };
        // No policy
        assert_eq!(decide(None, &input), Decision::Approve);
        // With policy that would otherwise deny
        let p = ToolPolicy {
            accept_edits: false,
            allow_commands: vec![],
            allow_mcp: vec![],
            deny: vec![],
        };
        assert_eq!(decide(Some(&p), &input), Decision::Approve);
    }

    // --- MCP tool allowed (lines 77-80) ---
    #[test]
    fn mcp_tool_allowed() {
        let p = ToolPolicy {
            accept_edits: false,
            allow_commands: vec![],
            allow_mcp: vec!["my_tool".into()],
            deny: vec![],
        };
        let input = PermissionInputs {
            mcp_tool: Some("my_tool".into()),
            ..Default::default()
        };
        assert_eq!(decide(Some(&p), &input), Decision::Approve);
    }

    // --- MCP tool denied (lines 77, 82-83) ---
    #[test]
    fn mcp_tool_denied() {
        let p = policy(false, &[], &[]);
        let input = PermissionInputs {
            mcp_tool: Some("unknown_tool".into()),
            ..Default::default()
        };
        assert!(matches!(decide(Some(&p), &input), Decision::Deny(_)));
    }

    // --- fallthrough to Decision::Approve (line 88) ---
    #[test]
    fn unknown_request_with_policy_approves() {
        let p = policy(false, &[], &[]);
        assert_eq!(
            decide(Some(&p), &PermissionInputs::default()),
            Decision::Approve
        );
    }

    // --- find_str_field: nested object (lines 113-119) ---
    #[test]
    fn find_str_field_nested_object() {
        let v: serde_json::Value = serde_json::json!({"outer": {"inner": {"target": "found"}}});
        assert_eq!(find_str_field(&v, "target"), Some("found".into()));
    }

    // --- find_str_field: array (line 121) ---
    #[test]
    fn find_str_field_in_array() {
        let v: serde_json::Value = serde_json::json!([{"k": "a"}, {"k": "b"}]);
        assert_eq!(find_str_field(&v, "k"), Some("a".into()));
    }

    // --- find_str_field: scalar returns None (line 122) ---
    #[test]
    fn find_str_field_scalar() {
        let v: serde_json::Value = serde_json::json!("string");
        assert_eq!(find_str_field(&v, "key"), None);
    }

    // --- find_str_field: key absent (line 119 fallthrough) ---
    #[test]
    fn find_str_field_missing_key() {
        let v: serde_json::Value = serde_json::json!({"a": 1, "b": 2});
        assert_eq!(find_str_field(&v, "missing"), None);
    }

    // --- extract_inputs: command (lines 96-110) ---
    #[test]
    fn extract_inputs_command() {
        let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "toolCall": {
                "toolCallId": "t1",
                "kind": "execute",
                "rawInput": { "command": "cargo build" }
            },
            "options": [],
            "_meta": null
        }))
        .unwrap();
        let inputs = extract_inputs(&req);
        assert_eq!(inputs.command.as_deref(), Some("cargo build"));
        assert!(!inputs.is_file_edit);
        assert!(inputs.mcp_tool.is_none());
    }

    // --- extract_inputs: file edit via kind=="edit" (lines 99-100) ---
    #[test]
    fn extract_inputs_file_edit_via_kind() {
        let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "toolCall": {
                "toolCallId": "t1",
                "kind": "edit",
                "rawInput": {}
            },
            "options": [],
            "_meta": null
        }))
        .unwrap();
        let inputs = extract_inputs(&req);
        assert!(inputs.is_file_edit);
        assert_eq!(inputs.command, None);
        assert!(inputs.mcp_tool.is_none());
    }

    // --- extract_inputs: file edit via write_text_file (lines 99-100) ---
    #[test]
    fn extract_inputs_file_edit_via_write_text_file() {
        let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "toolCall": {
                "toolCallId": "t1",
                "rawInput": { "write_text_file": {} }
            },
            "options": [],
            "_meta": null
        }))
        .unwrap();
        let inputs = extract_inputs(&req);
        assert!(inputs.is_file_edit);
    }

    // --- extract_inputs: MCP tool via "tool" field (lines 102-104) ---
    #[test]
    fn extract_inputs_mcp_tool() {
        let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "toolCall": {
                "toolCallId": "t1",
                "rawInput": { "tool": "my_mcp_tool" }
            },
            "options": [],
            "_meta": null
        }))
        .unwrap();
        let inputs = extract_inputs(&req);
        assert_eq!(inputs.mcp_tool.as_deref(), Some("my_mcp_tool"));
    }

    // --- extract_inputs: MCP tool via "name" fallback (lines 102-104) ---
    #[test]
    fn extract_inputs_mcp_tool_via_name() {
        let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "toolCall": {
                "toolCallId": "t1",
                "rawInput": { "name": "mcp_helper" }
            },
            "options": [],
            "_meta": null
        }))
        .unwrap();
        let inputs = extract_inputs(&req);
        assert_eq!(inputs.mcp_tool.as_deref(), Some("mcp_helper"));
    }

    // --- extract_inputs: MCP filter rejects non-mcp (lines 102-104 filter) ---
    #[test]
    fn extract_inputs_no_mcp_without_keyword() {
        let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "toolCall": {
                "toolCallId": "t1",
                "rawInput": { "tool": "some_tool" }
            },
            "options": [],
            "_meta": null
        }))
        .unwrap();
        let inputs = extract_inputs(&req);
        assert!(inputs.mcp_tool.is_none());
    }

    // --- parse_inputs_from_json with Value::Null (unwrap_or fallback, line 97) ---
    #[test]
    fn parse_inputs_from_json_null_fallback() {
        let inputs = parse_inputs_from_json(&serde_json::Value::Null);
        assert_eq!(inputs.command, None);
        assert!(!inputs.is_file_edit);
        assert_eq!(inputs.mcp_tool, None);
    }

    // --- deny list takes priority over file edit (lines 50-55 vs 57-65) ---
    #[test]
    fn deny_list_wins_over_file_edit() {
        let p = policy(true, &[], &["rm -rf"]);
        let input = PermissionInputs {
            command: Some("rm -rf /".into()),
            is_file_edit: true,
            ..Default::default()
        };
        // deny-list check runs first, so this is Deny even though accept_edits=true
        assert!(matches!(decide(Some(&p), &input), Decision::Deny(_)));
    }

    // --- extract_inputs: both command and mcp_tool (lines 96-110) ---
    #[test]
    fn extract_inputs_command_and_mcp_tool() {
        let req: RequestPermissionRequest = serde_json::from_value(serde_json::json!({
            "sessionId": "s1",
            "toolCall": {
                "toolCallId": "t1",
                "kind": "execute",
                "rawInput": {
                    "command": "cargo build",
                    "tool": "my_mcp",
                    "name": "my_mcp"
                }
            },
            "options": [],
            "_meta": null
        }))
        .unwrap();
        let inputs = extract_inputs(&req);
        assert_eq!(inputs.command.as_deref(), Some("cargo build"));
        assert!(!inputs.is_file_edit);
        // mcp_tool is set because raw contains "mcp"
        assert_eq!(inputs.mcp_tool.as_deref(), Some("my_mcp"));
    }
}
