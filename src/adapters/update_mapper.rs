//! Map ACP `SessionUpdate` notifications → Maestro [`ProgressDelta`] events,
//! while accumulating the agent's final message text and token usage.
//!
//! ACP schema types are macro-generated, so rather than depend on exact nested
//! field types we extract text/usage from the serialized JSON. Only the top-level
//! [`SessionUpdate`] variant names are matched directly.

use crate::core::contract::event::{AgentEvent, EventSender, ProgressDelta};
use crate::core::contract::ids::{AgentId, RunId, TokenUsage};
use agent_client_protocol::schema::SessionUpdate;
use serde::Serialize;
use std::sync::Mutex;

/// Shared sink for the streamed agent message + token usage of one run.
#[derive(Default)]
pub struct Accumulator {
    pub message: Mutex<String>,
    pub tokens: Mutex<TokenUsage>,
}

impl Accumulator {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Handle one streamed update: emit a progress event and update the accumulator.
pub fn handle_update(
    update: &SessionUpdate,
    run_id: RunId,
    agent_id: AgentId,
    acc: &Accumulator,
    events: &EventSender,
) {
    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let Some(text) = json_find_text(chunk) {
                acc.message.lock().unwrap().push_str(&text);
                emit(events, run_id, agent_id, ProgressDelta::Message { text });
            }
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            if let Some(text) = json_find_text(chunk) {
                emit(
                    events,
                    run_id,
                    agent_id,
                    ProgressDelta::Message {
                        text: format!("[reasoning] {text}"),
                    },
                );
            }
        }
        SessionUpdate::ToolCall(tc) => {
            let v = to_json(tc);
            let name = find_str(&v, "title").unwrap_or_else(|| "tool".to_string());
            let summary = find_str(&v, "kind").unwrap_or_default();
            emit(events, run_id, agent_id, ProgressDelta::ToolCall { name, summary });
        }
        SessionUpdate::ToolCallUpdate(u) => {
            let v = to_json(u);
            if let Some(path) = find_str(&v, "path") {
                emit(
                    events,
                    run_id,
                    agent_id,
                    ProgressDelta::FileEdit { path: path.into() },
                );
            }
        }
        _ => {}
    }

    // Best-effort: any update carrying a `usage` object updates token totals.
    if let Some(usage) = extract_usage(update) {
        *acc.tokens.lock().unwrap() = usage;
        emit(events, run_id, agent_id, ProgressDelta::Tokens { usage });
    }
}

fn emit(events: &EventSender, run_id: RunId, agent_id: AgentId, delta: ProgressDelta) {
    let _ = events.send(AgentEvent::AgentProgress {
        run_id,
        agent_id,
        delta,
    });
}

fn to_json<T: Serialize>(t: &T) -> serde_json::Value {
    serde_json::to_value(t).unwrap_or(serde_json::Value::Null)
}

/// Find the first `"text"` string value anywhere in a serializable value.
fn json_find_text<T: Serialize>(t: &T) -> Option<String> {
    find_str(&to_json(t), "text")
}

fn find_str(v: &serde_json::Value, key: &str) -> Option<String> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get(key) {
                return Some(s.clone());
            }
            map.values().find_map(|child| find_str(child, key))
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(|child| find_str(child, key)),
        _ => None,
    }
}

/// Extract a `TokenUsage` from a `usage` object embedded in the update, if any.
fn extract_usage(update: &SessionUpdate) -> Option<TokenUsage> {
    let v = to_json(update);
    let usage = find_object(&v, "usage")?;
    let get = |k: &str| usage.get(k).and_then(|n| n.as_u64()).unwrap_or(0);
    let (input, output) = (
        get("input_tokens").max(get("input")),
        get("output_tokens").max(get("output")),
    );
    if input == 0 && output == 0 {
        return None;
    }
    Some(TokenUsage {
        input,
        output,
        cache_read: get("cache_read_tokens").max(get("cache_read")),
        cache_write: get("cache_write_tokens").max(get("cache_write")),
    })
}

fn find_object<'a>(
    v: &'a serde_json::Value,
    key: &str,
) -> Option<&'a serde_json::Map<String, serde_json::Value>> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Object(o)) = map.get(key) {
                return Some(o);
            }
            map.values().find_map(|child| find_object(child, key))
        }
        serde_json::Value::Array(arr) => arr.iter().find_map(|child| find_object(child, key)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_str_digs_nested() {
        let v = serde_json::json!({ "a": { "b": { "text": "hi" } } });
        assert_eq!(find_str(&v, "text").as_deref(), Some("hi"));
    }

    #[test]
    fn extract_usage_reads_input_output() {
        // SessionUpdate is opaque here; test the JSON digger directly.
        let v = serde_json::json!({ "wrap": { "usage": { "input": 5, "output": 7 } } });
        let o = find_object(&v, "usage").unwrap();
        assert_eq!(o.get("input").unwrap().as_u64(), Some(5));
    }
}
