//! Agent task construction and result-table helpers.
//!
//! Shared by `agent()` and `parallel()`: both turn a Lua opts table into an
//! [`AgentTask`] (plus cache key + optional backend id) and turn a scheduler
//! result back into the Lua result table handed to the workflow.

use crate::core::contract::backend::AgentTask;
use crate::core::contract::finding::Finding;
use crate::core::contract::ids::PhaseId;
use crate::core::journal::AgentCacheKey;
use crate::runtime::sdk::convert::{lua_value_from_json, value_to_json};
use mlua::{Lua, Table, Value};
use std::path::PathBuf;
use std::time::Duration;

/// Build an [`AgentTask`] (+ cache key + optional backend id) from a Lua opts
/// table. Recognised keys: `prompt` (required), `model`, `schema`, `backend`,
/// `timeout_ms` (idle timeout: max silence from the agent before the session
/// is killed).
pub(crate) fn build_task(
    opts: &Table,
    phase_id: PhaseId,
) -> mlua::Result<(AgentTask, AgentCacheKey, Option<String>)> {
    let prompt: String = opts
        .get("prompt")
        .map_err(|_| mlua::Error::RuntimeError("agent: missing required 'prompt' field".into()))?;
    let model: Option<String> = opts.get::<Option<String>>("model").ok().flatten();
    let backend: Option<String> = opts.get::<Option<String>>("backend").ok().flatten();
    let description: Option<String> = opts.get::<Option<String>>("description").ok().flatten();
    let role: Option<String> = opts.get::<Option<String>>("role").ok().flatten();
    let timeout = opts
        .get::<i64>("timeout_ms")
        .ok()
        .filter(|v| *v > 0)
        .map(|v| Duration::from_millis(v as u64));
    let output_schema = match opts.get::<Value>("schema") {
        Ok(Value::Table(t)) => Some(value_to_json(Value::Table(t))?),
        Ok(Value::Boolean(b)) => Some(serde_json::Value::Bool(b)),
        _ => None,
    };

    let prompt = match &output_schema {
        Some(_) => format!(
            "{prompt}\n\n\
             ---\n\
             IMPORTANT: You MUST call the `structured_output` tool to submit your result.\n\
             Do NOT return the result as a text message. Call the tool.",
            prompt = prompt,
        ),
        None => prompt,
    };

    let cache_key = AgentCacheKey::new(&prompt, model.as_deref(), phase_id);
    let task = AgentTask {
        agent_id: uuid::Uuid::now_v7(),
        phase_id,
        prompt,
        model,
        description,
        role,
        allowlist: None,
        workdir: PathBuf::from("."),
        mcp_endpoint: None,
        timeout,
        output_schema,
    };
    Ok((task, cache_key, backend))
}

/// Build the Lua result table returned to workflows by `agent()`/`parallel()`.
/// Fields: `status`, `ok`, `output`, `tokens`, `findings`.
pub(crate) fn build_result_table(
    lua: &Lua,
    status: &str,
    output: serde_json::Value,
    tokens: u64,
    findings: &[Finding],
) -> mlua::Result<Table> {
    let t = lua.create_table()?;
    t.set("status", status)?;
    t.set("ok", status == "ok")?;
    t.set("output", lua_value_from_json(lua, output)?)?;
    t.set("tokens", tokens as i64)?;
    let ft = lua.create_table()?;
    for (i, f) in findings.iter().enumerate() {
        let e = lua.create_table()?;
        e.set("kind", f.kind.as_str())?;
        e.set("severity", format!("{:?}", f.severity).to_lowercase())?;
        e.set("title", f.title.as_str())?;
        e.set("detail", f.detail.as_str())?;
        ft.set(i + 1, e)?;
    }
    t.set("findings", ft)?;
    Ok(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::finding::Severity;
    use mlua::Lua;

    fn opts(lua: &Lua) -> Table {
        lua.create_table().unwrap()
    }

    #[test]
    fn build_task_requires_prompt() {
        let lua = Lua::new();
        assert!(build_task(&opts(&lua), 1).is_err());
    }

    #[test]
    fn build_task_parses_recognised_fields() {
        let lua = Lua::new();
        let o = opts(&lua);
        o.set("prompt", "do it").unwrap();
        o.set("model", "claude-x").unwrap();
        o.set("backend", "acp").unwrap();
        o.set("timeout_ms", 5000).unwrap();
        let (task, _key, backend) = build_task(&o, 7).unwrap();
        assert_eq!(task.prompt, "do it");
        assert_eq!(task.model.as_deref(), Some("claude-x"));
        assert_eq!(task.phase_id, 7);
        assert_eq!(task.timeout, Some(Duration::from_millis(5000)));
        assert_eq!(backend.as_deref(), Some("acp"));
    }

    #[test]
    fn build_task_drops_nonpositive_timeout() {
        let lua = Lua::new();
        let o = opts(&lua);
        o.set("prompt", "x").unwrap();
        o.set("timeout_ms", 0).unwrap();
        let (task, _, _) = build_task(&o, 0).unwrap();
        assert!(task.timeout.is_none());
    }

    #[test]
    fn build_task_schema_injects_tool_call_instruction() {
        let lua = Lua::new();
        let o = opts(&lua);
        o.set("prompt", "analyze").unwrap();
        let schema = lua.create_table().unwrap();
        schema.set("type", "object").unwrap();
        let props = lua.create_table().unwrap();
        props.set("x", lua.create_table().unwrap()).unwrap();
        schema.set("properties", props).unwrap();
        o.set("schema", schema).unwrap();

        let (task, _, _) = build_task(&o, 0).unwrap();
        assert!(task.prompt.contains("IMPORTANT"));
        assert!(task.prompt.contains("structured_output"));
        assert!(task.prompt.contains("tool"));
        assert!(!task.prompt.contains("JSON Schema"));
        assert!(task.output_schema.is_some());
    }

    #[test]
    fn build_task_no_schema_keeps_prompt_clean() {
        let lua = Lua::new();
        let o = opts(&lua);
        o.set("prompt", "just text").unwrap();
        let (task, _, _) = build_task(&o, 0).unwrap();
        assert_eq!(task.prompt, "just text");
        assert!(task.output_schema.is_none());
    }

    #[test]
    fn build_task_cache_key_is_deterministic() {
        let lua = Lua::new();
        let key = |phase| {
            let o = opts(&lua);
            o.set("prompt", "same prompt").unwrap();
            o.set("model", "m").unwrap();
            build_task(&o, phase).unwrap().1.hash
        };
        // Same prompt+model+phase → same hash; different phase → different hash.
        assert_eq!(key(3), key(3));
        assert_ne!(key(3), key(4));
    }

    #[test]
    fn result_table_ok_flag_and_findings_shape() {
        let lua = Lua::new();
        let findings = vec![Finding {
            kind: "missing_auth".into(),
            severity: Severity::High,
            title: "t".into(),
            detail: "d".into(),
            location: None,
            evidence: vec![],
            data: serde_json::Value::Null,
        }];
        let t = build_result_table(&lua, "ok", serde_json::json!({ "x": 1 }), 42, &findings).unwrap();
        assert_eq!(t.get::<String>("status").unwrap(), "ok");
        assert!(t.get::<bool>("ok").unwrap());
        assert_eq!(t.get::<i64>("tokens").unwrap(), 42);
        let ft: Table = t.get("findings").unwrap();
        assert_eq!(ft.raw_len(), 1);
        let f0: Table = ft.get(1).unwrap();
        assert_eq!(f0.get::<String>("kind").unwrap(), "missing_auth");
        assert_eq!(f0.get::<String>("severity").unwrap(), "high");
    }

    #[test]
    fn result_table_ok_false_for_non_ok_status() {
        let lua = Lua::new();
        let t = build_result_table(&lua, "error", serde_json::Value::Null, 0, &[]).unwrap();
        assert!(!t.get::<bool>("ok").unwrap());
    }
}
