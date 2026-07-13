//! Map ACP `SessionUpdate` notifications → Luft [`ProgressDelta`] events,
//! while accumulating the agent's final message text and token usage.
//!
//! ACP schema types are macro-generated, so rather than depend on exact nested
//! field types we extract text/usage from the serialized JSON. Only the top-level
//! [`SessionUpdate`] variant names are matched directly.

use agent_client_protocol::schema::SessionUpdate;
use luft_core::contract::event::{AgentEvent, EventSender, ProgressDelta};
use luft_core::contract::ids::{AgentId, RunId, TokenUsage};
use serde::Serialize;
use std::sync::Mutex;

/// Shared sink for the streamed agent message + token usage of one run.
#[derive(Default)]
pub struct Accumulator {
    pub message: Mutex<String>,
    pub tokens: Mutex<TokenUsage>,
    pub structured_output: Mutex<Option<serde_json::Value>>,
}

impl Accumulator {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Handle one streamed update: emit a progress event and update the accumulator.
///
/// When `emit_raw` is set, the verbatim `SessionUpdate` is additionally emitted
/// as [`AgentEvent::AcpRaw`] before the (lossy) projection — this captures even
/// the variants the projection drops.
pub fn handle_update(
    update: &SessionUpdate,
    run_id: RunId,
    agent_id: AgentId,
    acc: &Accumulator,
    events: &EventSender,
    emit_raw: bool,
) {
    if emit_raw {
        let raw = to_json(update);
        let kind = raw
            .get("sessionUpdate")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        tracing::trace!(%run_id, %agent_id, %kind, "ACP raw update");
        let _ = events.send(AgentEvent::AcpRaw {
            run_id,
            agent_id,
            kind,
            raw,
        });
    }

    match update {
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let Some(text) = json_find_text(chunk) {
                tracing::debug!(text_len = text.len(), "ACP agent_message_chunk");
                acc.message.lock().unwrap().push_str(&text);
                emit(events, run_id, agent_id, ProgressDelta::Message { text });
            }
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            if let Some(text) = json_find_text(chunk) {
                tracing::debug!(text_len = text.len(), "ACP agent_thought_chunk");
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
            let title = find_str(&v, "title")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "tool".to_string());
            let kind = find_str(&v, "kind").unwrap_or_default();

            tracing::debug!(title = %title, kind = %kind, "ACP tool_call");

            if title.contains("structured_output") {
                if let Some(raw_input) = v
                    .get("rawInput")
                    .cloned()
                    .or_else(|| v.get("raw_input").cloned())
                {
                    if !raw_input.is_null()
                        && !raw_input.as_object().map(|o| o.is_empty()).unwrap_or(false)
                    {
                        tracing::debug!(title = %title, "captured structured_output rawInput from ToolCall");
                        *acc.structured_output.lock().unwrap() = Some(raw_input);
                    }
                }
            }

            emit(
                events,
                run_id,
                agent_id,
                ProgressDelta::ToolCall {
                    name: title,
                    summary: kind,
                },
            );
        }
        SessionUpdate::ToolCallUpdate(u) => {
            let v = to_json(u);

            tracing::debug!(title = %find_str(&v, "title").unwrap_or_default(), path = ?find_str(&v, "path"), "ACP tool_call_update");

            if title_contains(&v, "structured_output") {
                if let Some(raw_input) = v
                    .get("rawInput")
                    .cloned()
                    .or_else(|| v.get("raw_input").cloned())
                {
                    if !raw_input.is_null()
                        && !raw_input.as_object().map(|o| o.is_empty()).unwrap_or(false)
                    {
                        tracing::debug!("captured structured_output rawInput from ToolCallUpdate");
                        *acc.structured_output.lock().unwrap() = Some(raw_input);
                    }
                }
            }

            if let Some(path) = find_str(&v, "path") {
                emit(
                    events,
                    run_id,
                    agent_id,
                    ProgressDelta::FileEdit { path: path.into() },
                );
            }
        }
        SessionUpdate::UsageUpdate(u) => {
            tracing::debug!(
                used = u.used,
                size = u.size,
                cost = ?u.cost,
                "ACP usage_update"
            );
        }
        other => {
            let kind = serde_json::to_value(other)
                .ok()
                .and_then(|v| {
                    v.get("sessionUpdate")
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| "unknown".to_string());
            tracing::debug!(%kind, "ACP unhandled session/update");
        }
    }

    // Best-effort: any update carrying a `usage` object updates token totals.
    if let Some(usage) = extract_usage(update) {
        tracing::debug!(
            input = usage.input,
            output = usage.output,
            "token usage update"
        );
        *acc.tokens.lock().unwrap() = usage;
        emit(events, run_id, agent_id, ProgressDelta::Tokens { usage });
    } else {
        tracing::trace!("no usage data in update");
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

/// Find the first `\"text\"` string value anywhere in a serializable value.
fn json_find_text<T: Serialize>(t: &T) -> Option<String> {
    find_str(&to_json(t), "text")
}

/// Check if the `title` field in a JSON value contains a substring.
fn title_contains(v: &serde_json::Value, needle: &str) -> bool {
    v.get("title")
        .and_then(|t| t.as_str())
        .map(|s| s.contains(needle))
        .unwrap_or(false)
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
/// Handles both snake_case (`input_tokens`) and camelCase (`inputTokens`) keys,
/// since ACP uses camelCase serialization but some agents may use snake_case.
fn extract_usage(update: &SessionUpdate) -> Option<TokenUsage> {
    let v = to_json(update);
    let usage = find_object(&v, "usage").or_else(|| find_object(&v, "token_usage"))?;
    let get = |snake: &str, camel: &str| {
        usage
            .get(snake)
            .or_else(|| usage.get(camel))
            .and_then(|n| n.as_u64())
            .unwrap_or(0)
    };
    let (input, output) = (
        get("input_tokens", "inputTokens").max(get("input", "input")),
        get("output_tokens", "outputTokens").max(get("output", "output")),
    );
    if input == 0 && output == 0 {
        return None;
    }
    let cached = get("cached_tokens", "cachedTokens");
    Some(TokenUsage {
        input,
        output,
        cache_read: get("cache_read_tokens", "cachedReadTokens")
            .max(get("cache_read", "cached_read"))
            .max(cached),
        cache_write: get("cache_write_tokens", "cachedWriteTokens")
            .max(get("cache_write", "cached_write")),
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

    #[tokio::test]
    async fn emit_sends_correct_agent_event() {
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(16);
        let run_id = RunId::now_v7();
        let agent_id = AgentId::now_v7();
        let delta = ProgressDelta::Message {
            text: "test".to_string(),
        };

        emit(&events_tx, run_id, agent_id, delta);

        let event = events_rx.recv().await.unwrap();
        match event {
            AgentEvent::AgentProgress {
                run_id: r_id,
                agent_id: a_id,
                delta: d,
            } => {
                assert_eq!(r_id, run_id);
                assert_eq!(a_id, agent_id);
                match d {
                    ProgressDelta::Message { text } => {
                        assert_eq!(text, "test");
                    }
                    _ => panic!("Expected Message delta"),
                }
            }
            _ => panic!("Expected AgentProgress event"),
        }
    }

    #[test]
    fn find_str_top_level() {
        let v = serde_json::json!({ "text": "top level" });
        assert_eq!(find_str(&v, "text").as_deref(), Some("top level"));
    }

    #[test]
    fn find_str_array_search() {
        let v = serde_json::json!({ "items": [{ "text": "first" }, { "text": "second" }] });
        assert_eq!(find_str(&v, "text").as_deref(), Some("first"));
    }

    #[test]
    fn find_str_no_match() {
        let v = serde_json::json!({ "a": { "b": { "other": "hi" } } });
        assert!(find_str(&v, "text").is_none());
    }

    #[test]
    fn find_object_top_level() {
        let v = serde_json::json!({ "usage": { "input": 10, "output": 5 } });
        let o = find_object(&v, "usage").unwrap();
        assert_eq!(o.get("input").unwrap().as_u64(), Some(10));
    }

    #[test]
    fn find_object_nested() {
        let v = serde_json::json!({ "wrap": { "usage": { "input": 15 } } });
        let o = find_object(&v, "usage").unwrap();
        assert_eq!(o.get("input").unwrap().as_u64(), Some(15));
    }

    #[test]
    fn find_object_no_match() {
        let v = serde_json::json!({ "a": { "b": { "other": "data" } } });
        assert!(find_object(&v, "usage").is_none());
    }

    #[tokio::test]
    async fn accumulator_is_default() {
        let acc = Accumulator::new();
        assert!(acc.message.lock().unwrap().is_empty());
        assert_eq!(acc.tokens.lock().unwrap().input, 0);
        assert_eq!(acc.tokens.lock().unwrap().output, 0);
    }

    #[tokio::test]
    async fn accumulator_accumulates_text() {
        let acc = Accumulator::new();
        acc.message.lock().unwrap().push_str("Hello");
        acc.message.lock().unwrap().push_str(" World");
        assert_eq!(acc.message.lock().unwrap().as_str(), "Hello World");
    }

    #[tokio::test]
    async fn accumulator_tracks_tokens() {
        let acc = Accumulator::new();
        *acc.tokens.lock().unwrap() = TokenUsage {
            input: 100,
            output: 50,
            cache_read: 10,
            cache_write: 5,
        };
        assert_eq!(acc.tokens.lock().unwrap().input, 100);
        assert_eq!(acc.tokens.lock().unwrap().output, 50);
    }

    #[test]
    fn extract_usage_from_json_with_tokens() {
        // Test extract_usage logic directly on JSON
        let v = serde_json::json!({
            "content": [{ "text": "Hello" }],
            "usage": { "input_tokens": 10, "output_tokens": 5 }
        });
        let usage = find_object(&v, "usage").and_then(|o| {
            let get = |k: &str| o.get(k).and_then(|n| n.as_u64()).unwrap_or(0);
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
        });

        assert!(usage.is_some());
        let usage = usage.unwrap();
        assert_eq!(usage.input, 10);
        assert_eq!(usage.output, 5);
    }

    #[test]
    fn extract_usage_alternative_field_names() {
        let v = serde_json::json!({
            "usage": { "input": 15, "output": 20, "cache_read": 5, "cache_write": 3 }
        });
        let usage = find_object(&v, "usage").and_then(|o| {
            let get = |k: &str| o.get(k).and_then(|n| n.as_u64()).unwrap_or(0);
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
        });

        assert!(usage.is_some());
        let usage = usage.unwrap();
        assert_eq!(usage.input, 15);
        assert_eq!(usage.output, 20);
        assert_eq!(usage.cache_read, 5);
        assert_eq!(usage.cache_write, 3);
    }

    #[test]
    fn handle_update_message_chunk_accumulates_and_emits() {
        use agent_client_protocol::schema::{ContentBlock, ContentChunk, TextContent};

        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("hello")));
        let update = SessionUpdate::AgentMessageChunk(chunk);
        let acc = Accumulator::new();
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);

        assert_eq!(acc.message.lock().unwrap().as_str(), "hello");
        let evt = rx.try_recv().unwrap();
        match evt {
            AgentEvent::AgentProgress { delta, .. } => match delta {
                ProgressDelta::Message { text } => assert_eq!(text, "hello"),
                _ => panic!("expected Message"),
            },
            _ => panic!("expected AgentProgress"),
        }
    }

    #[test]
    fn handle_update_thought_chunk_emits_reasoning_no_accumulate() {
        use agent_client_protocol::schema::{ContentBlock, ContentChunk, TextContent};

        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("thinking")));
        let update = SessionUpdate::AgentThoughtChunk(chunk);
        let acc = Accumulator::new();
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);

        assert!(acc.message.lock().unwrap().is_empty());
        let evt = rx.try_recv().unwrap();
        match evt {
            AgentEvent::AgentProgress { delta, .. } => match delta {
                ProgressDelta::Message { text } => {
                    assert!(text.starts_with("[reasoning]"));
                    assert!(text.contains("thinking"));
                }
                _ => panic!("expected Message"),
            },
            _ => panic!("expected AgentProgress"),
        }
    }

    #[test]
    fn handle_update_tool_call_with_title() {
        let tc = agent_client_protocol::schema::ToolCall::new("tc-1", "ReadFile");
        let update = SessionUpdate::ToolCall(tc);
        let acc = Accumulator::new();
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);

        let evt = rx.try_recv().unwrap();
        match evt {
            AgentEvent::AgentProgress { delta, .. } => match delta {
                ProgressDelta::ToolCall { name, .. } => assert_eq!(name, "ReadFile"),
                _ => panic!("expected ToolCall"),
            },
            _ => panic!("expected AgentProgress"),
        }
    }

    #[test]
    fn handle_update_structured_output_tool_call_captures_value() {
        let raw = serde_json::json!({
            "file": "src/adapters/result_collector.rs",
            "kind": "rust",
            "summary": "collects agent results"
        });
        let mut tc = agent_client_protocol::schema::ToolCall::new("tc-1", "structured_output");
        tc.raw_input = Some(raw.clone());

        let update = SessionUpdate::ToolCall(tc);
        let acc = Accumulator::new();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);

        let captured = acc.structured_output.lock().unwrap().clone();
        assert!(captured.is_some(), "structured_output should be captured");
        assert_eq!(captured.unwrap(), raw);
    }

    #[test]
    fn handle_update_structured_output_tool_call_update_captures_value() {
        let raw = serde_json::json!({
            "file": "src/adapters/result_collector.rs",
            "kind": "rust",
            "summary": "collects agent results"
        });
        let fields = agent_client_protocol::schema::ToolCallUpdateFields::default()
            .title("structured_output")
            .raw_input(raw.clone());
        let u = agent_client_protocol::schema::ToolCallUpdate::new("tc-so", fields);
        let update = SessionUpdate::ToolCallUpdate(u);
        let acc = Accumulator::new();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);

        let captured = acc.structured_output.lock().unwrap().clone();
        assert!(
            captured.is_some(),
            "structured_output should be captured via ToolCallUpdate"
        );
        assert_eq!(captured.unwrap(), raw);
    }

    #[test]
    fn handle_update_structured_output_empty_raw_input_not_captured() {
        let mut tc = agent_client_protocol::schema::ToolCall::new("tc-1", "structured_output");
        tc.raw_input = Some(serde_json::json!({}));

        let update = SessionUpdate::ToolCall(tc);
        let acc = Accumulator::new();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);

        let captured = acc.structured_output.lock().unwrap().clone();
        assert!(captured.is_none(), "empty rawInput should not be captured");
    }

    #[test]
    fn handle_update_non_structured_tool_call_does_not_capture() {
        let mut tc = agent_client_protocol::schema::ToolCall::new("tc-1", "ReadFile");
        tc.raw_input = Some(serde_json::json!({"path": "src/main.rs"}));

        let update = SessionUpdate::ToolCall(tc);
        let acc = Accumulator::new();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);

        let captured = acc.structured_output.lock().unwrap().clone();
        assert!(
            captured.is_none(),
            "non-structured_output tool should not capture"
        );
    }

    #[test]
    fn handle_update_tool_call_empty_title_uses_default() {
        let tc = agent_client_protocol::schema::ToolCall::new("tc-2", "");
        let update = SessionUpdate::ToolCall(tc);
        let acc = Accumulator::new();
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);

        let evt = rx.try_recv().unwrap();
        match evt {
            AgentEvent::AgentProgress { delta, .. } => match delta {
                ProgressDelta::ToolCall { name, .. } => assert_eq!(name, "tool"),
                _ => panic!("expected ToolCall"),
            },
            _ => panic!("expected AgentProgress"),
        }
    }

    #[test]
    fn handle_update_tool_call_update_with_path() {
        let loc = agent_client_protocol::schema::ToolCallLocation::new("src/main.rs");
        let fields =
            agent_client_protocol::schema::ToolCallUpdateFields::default().locations(vec![loc]);
        let u = agent_client_protocol::schema::ToolCallUpdate::new("tc-3", fields);
        let update = SessionUpdate::ToolCallUpdate(u);
        let acc = Accumulator::new();
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);

        let evt = rx.try_recv().unwrap();
        match evt {
            AgentEvent::AgentProgress { delta, .. } => match delta {
                ProgressDelta::FileEdit { path } => {
                    assert_eq!(path.to_str().unwrap(), "src/main.rs");
                }
                _ => panic!("expected FileEdit"),
            },
            _ => panic!("expected AgentProgress"),
        }
    }

    #[test]
    fn handle_update_tool_call_update_no_path_no_emit() {
        let fields = agent_client_protocol::schema::ToolCallUpdateFields::default();
        let u = agent_client_protocol::schema::ToolCallUpdate::new("tc-4", fields);
        let update = SessionUpdate::ToolCallUpdate(u);
        let acc = Accumulator::new();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);
        assert!(tx.is_empty());
    }

    #[test]
    fn handle_update_multiple_chunks_accumulate() {
        use agent_client_protocol::schema::{ContentBlock, ContentChunk, TextContent};

        let acc = Accumulator::new();
        let (tx, _rx) = tokio::sync::broadcast::channel(16);

        for t in ["hello ", "world", "!"] {
            let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new(t)));
            handle_update(
                &SessionUpdate::AgentMessageChunk(chunk),
                RunId::nil(),
                AgentId::nil(),
                &acc,
                &tx,
                false,
            );
        }

        assert_eq!(acc.message.lock().unwrap().as_str(), "hello world!");
    }

    #[test]
    fn handle_update_emit_raw_prepends_acp_raw() {
        use agent_client_protocol::schema::{ContentBlock, ContentChunk, TextContent};

        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("hi")));
        let update = SessionUpdate::AgentMessageChunk(chunk);
        let acc = Accumulator::new();
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, true);

        // Raw event comes first, carrying the discriminator + verbatim payload.
        match rx.try_recv().unwrap() {
            AgentEvent::AcpRaw { kind, raw, .. } => {
                assert_eq!(kind, "agent_message_chunk");
                assert_eq!(
                    raw.get("sessionUpdate").and_then(|v| v.as_str()),
                    Some("agent_message_chunk")
                );
            }
            other => panic!("expected AcpRaw, got {other:?}"),
        }
        // Projection still follows.
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentEvent::AgentProgress { .. }
        ));
    }

    #[test]
    fn handle_update_emit_raw_false_emits_no_acp_raw() {
        use agent_client_protocol::schema::{ContentBlock, ContentChunk, TextContent};

        let chunk = ContentChunk::new(ContentBlock::Text(TextContent::new("hi")));
        let update = SessionUpdate::AgentMessageChunk(chunk);
        let acc = Accumulator::new();
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, false);

        // Only the projected AgentProgress, never an AcpRaw.
        assert!(matches!(
            rx.try_recv().unwrap(),
            AgentEvent::AgentProgress { .. }
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn handle_update_emit_raw_captures_dropped_variant() {
        // `Plan` is not projected (`_ => {}`), but raw must still surface it.
        let plan = agent_client_protocol::schema::Plan::new(vec![]);
        let update = SessionUpdate::Plan(plan);
        let acc = Accumulator::new();
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);

        handle_update(&update, RunId::nil(), AgentId::nil(), &acc, &tx, true);

        match rx.try_recv().unwrap() {
            AgentEvent::AcpRaw { kind, .. } => assert_eq!(kind, "plan"),
            other => panic!("expected AcpRaw, got {other:?}"),
        }
        // No projection event for Plan.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn extract_usage_zero_tokens_returns_none() {
        let v = serde_json::json!({
            "content": [{ "text": "Hello" }],
            "usage": { "input_tokens": 0, "output_tokens": 0 }
        });
        let usage = find_object(&v, "usage").and_then(|o| {
            let get = |k: &str| o.get(k).and_then(|n| n.as_u64()).unwrap_or(0);
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
        });

        assert!(usage.is_none());
    }
}
