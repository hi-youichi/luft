//! Non-interactive permission decisions for ACP tool requests.
//!
//! v0.1 never blocks on a human: every `session/request_permission` is decided
//! synchronously from the task's [`ToolPolicy`]. The decision logic ([`decide`])
//! is pure and unit-tested; [`extract_inputs`] adapts an ACP request into the
//! pure inputs.

use crate::core::contract::backend::ToolPolicy;
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
        return if policy.allow_commands.iter().any(|p| cmd.starts_with(p.as_str())) {
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
    let raw = v.to_string();
    let is_file_edit = find_str_field(&v, "kind").as_deref() == Some("edit")
        || raw.contains("write_text_file");
    let command = find_str_field(&v, "command");
    let mcp_tool = find_str_field(&v, "tool")
        .or_else(|| find_str_field(&v, "name"))
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
        assert_eq!(decide(None, &PermissionInputs::default()), Decision::Approve);
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
        let edit = PermissionInputs { is_file_edit: true, ..Default::default() };
        assert_eq!(decide(Some(&policy(true, &[], &[])), &edit), Decision::Approve);
        assert!(matches!(
            decide(Some(&policy(false, &[], &[])), &edit),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn allow_commands_prefix() {
        let p = policy(false, &["cargo"], &[]);
        let ok = PermissionInputs { command: Some("cargo test".into()), ..Default::default() };
        let bad = PermissionInputs { command: Some("curl evil".into()), ..Default::default() };
        assert_eq!(decide(Some(&p), &ok), Decision::Approve);
        assert!(matches!(decide(Some(&p), &bad), Decision::Deny(_)));
    }
}
