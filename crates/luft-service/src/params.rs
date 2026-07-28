//! Shared parameter parsing for workflow tool consumers.
//!
//! Both Loom's `tool-workflow` and Luft's `luft-mcp` expose tools that accept
//! the same JSON arguments (`concurrency`, `offset`, `events_limit`, `types`,
//! `agent_id`). This module provides a single source of truth for parsing
//! and validating those parameters, eliminating ~140 lines of duplication.

use serde_json::Value;

use crate::json_to_lua::json_to_lua;

// ── Constants ──────────────────────────────────────────────────────────

pub const MIN_CONCURRENCY: usize = 1;
pub const MAX_CONCURRENCY: usize = 64;
pub const DEFAULT_EVENTS_LIMIT: u64 = 50;
pub const MAX_EVENTS_LIMIT: u64 = 500;

// ── Concurrency ───────────────────────────────────────────────────────

/// Parse the optional `concurrency` argument.
///
/// Returns `Ok(None)` when the argument is absent or null (caller decides
/// the default). Returns `Ok(Some(n))` for a valid integer in
/// `[MIN_CONCURRENCY, MAX_CONCURRENCY]`.
pub fn parse_concurrency(args: &Value) -> Result<Option<usize>, String> {
    let Some(v) = args.get("concurrency") else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let n = v
        .as_u64()
        .ok_or_else(|| format!("'concurrency' must be a positive integer, got {v}"))?;
    if !(MIN_CONCURRENCY as u64..=MAX_CONCURRENCY as u64).contains(&n) {
        return Err(format!(
            "'concurrency' must be between {MIN_CONCURRENCY} and {MAX_CONCURRENCY}, got {n}"
        ));
    }
    Ok(Some(n as usize))
}

// ── Events filter ─────────────────────────────────────────────────────

/// Parsed event-query parameters: `offset`, `events_limit`, optional
/// `types[]` filter, optional `agent_id` filter.
#[derive(Debug, Clone)]
pub struct EventsFilter {
    pub offset: u64,
    pub events_limit: u64,
    pub types: Option<Vec<String>>,
    pub agent_id: Option<String>,
}

impl EventsFilter {
    /// Parse all four event-query parameters from a JSON args object.
    pub fn from_args(args: &Value) -> Self {
        Self {
            offset: parse_events_offset(args),
            events_limit: parse_events_limit(args),
            types: parse_events_types(args),
            agent_id: parse_events_agent_id(args),
        }
    }

    /// Returns `true` when the event's `type` and `agent_id` match the
    /// filter criteria (or when no filter is set).
    pub fn matches(&self, event: &Value) -> bool {
        let type_ok = self.types.as_ref().is_none_or(|ts| {
            event
                .get("type")
                .and_then(|t| t.as_str())
                .map(|t| ts.iter().any(|x| x == t))
                .unwrap_or(false)
        });
        let agent_ok = self.agent_id.as_ref().is_none_or(|aid| {
            event
                .get("agent_id")
                .and_then(|a| a.as_str())
                .map(|a| a == aid)
                .unwrap_or(false)
        });
        type_ok && agent_ok
    }
}

/// Parse `offset` (default 0).
pub fn parse_events_offset(args: &Value) -> u64 {
    args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0)
}

/// Parse `events_limit` (default 50, clamped to `[1, 500]`).
pub fn parse_events_limit(args: &Value) -> u64 {
    args.get("events_limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_EVENTS_LIMIT)
        .clamp(1, MAX_EVENTS_LIMIT)
}

/// Parse `types[]` into a `Vec<String>`. Returns `None` when absent or empty.
pub fn parse_events_types(args: &Value) -> Option<Vec<String>> {
    let v = args.get("types")?;
    if v.is_null() {
        return None;
    }
    let arr = v.as_array()?;
    let out: Vec<String> = arr
        .iter()
        .filter_map(|t| t.as_str().map(String::from))
        .collect();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Parse `agent_id`. Returns `None` when absent or null.
pub fn parse_events_agent_id(args: &Value) -> Option<String> {
    let v = args.get("agent_id")?;
    if v.is_null() {
        return None;
    }
    v.as_str().map(String::from)
}

// ── Pagination ────────────────────────────────────────────────────────

/// Apply offset/limit pagination to a slice and compute `next_offset`.
///
/// Returns `(page, total_matching, next_offset)`.
pub fn paginate<T: Clone>(items: &[T], offset: u64, limit: u64) -> (Vec<T>, u64, Option<u64>) {
    let total = items.len() as u64;
    let page: Vec<T> = items
        .iter()
        .skip(offset as usize)
        .take(limit as usize)
        .cloned()
        .collect();
    let next_offset = if offset + (page.len() as u64) < total {
        Some(offset + page.len() as u64)
    } else {
        None
    };
    (page, total, next_offset)
}

/// Cursor-based pagination for list endpoints.
///
/// Finds the cursor in `items` (by comparing `key(item)` to `cursor`),
/// skips it, takes `limit` items, and returns `(page, next_cursor)`.
/// If `cursor` is `None`, starts from the beginning.
pub fn paginate_cursor<'a, T, F>(
    items: &'a [T],
    cursor: Option<&str>,
    limit: usize,
    key: F,
) -> (Vec<&'a T>, Option<String>)
where
    F: Fn(&T) -> &str,
{
    let start = match cursor {
        Some(c) => items
            .iter()
            .position(|item| key(item) == c)
            .map(|i| i + 1)
            .unwrap_or(0),
        None => 0,
    };

    let page: Vec<&T> = items.iter().skip(start).take(limit).collect();
    let next_cursor = if start + page.len() < items.len() {
        page.last().map(|item| key(item).to_string())
    } else {
        None
    };

    (page, next_cursor)
}

// ── List-query parameters ─────────────────────────────────────────────

/// Default page size for list endpoints.
pub const DEFAULT_LIST_LIMIT: u64 = 20;
/// Maximum page size for list endpoints.
pub const MAX_LIST_LIMIT: u64 = 100;
/// Valid status filter values (case-insensitive).
pub const STATUS_FILTERS: &[&str] = &["completed", "failed", "cancelled"];

/// Parse the `limit` argument for list endpoints (default 20, max 100).
pub fn parse_list_limit(args: &Value) -> Result<u64, String> {
    let Some(v) = args.get("limit") else {
        return Ok(DEFAULT_LIST_LIMIT);
    };
    if v.is_null() {
        return Ok(DEFAULT_LIST_LIMIT);
    }
    let n = v
        .as_u64()
        .ok_or_else(|| format!("'limit' must be a positive integer, got {v}"))?;
    if !(1..=MAX_LIST_LIMIT).contains(&n) {
        return Err(format!("'limit' must be between 1 and {MAX_LIST_LIMIT}, got {n}"));
    }
    Ok(n)
}

/// Parse the `status_filter` argument. Returns `None` when absent.
///
/// The returned string is lowercased and validated against [`STATUS_FILTERS`].
pub fn parse_status_filter(args: &Value) -> Result<Option<String>, String> {
    let Some(v) = args.get("status_filter") else {
        return Ok(None);
    };
    if v.is_null() {
        return Ok(None);
    }
    let s = v
        .as_str()
        .ok_or_else(|| format!("'status_filter' must be a string, got {v}"))?;
    let lower = s.to_lowercase();
    if !STATUS_FILTERS.contains(&lower.as_str()) {
        return Err(format!(
            "'status_filter' must be one of completed|failed|cancelled, got {s}"
        ));
    }
    Ok(Some(lower))
}

/// Parse the `cursor` argument. Returns `None` when absent, null, or empty.
pub fn parse_cursor(args: &Value) -> Option<String> {
    let v = args.get("cursor")?;
    if v.is_null() {
        return None;
    }
    v.as_str().filter(|s| !s.is_empty()).map(String::from)
}

// ── User args injection ───────────────────────────────────────────────

/// Extract the optional `args` parameter from a JSON args object.
///
/// Returns `None` when `args` is absent or null.
pub fn extract_user_args(args: &Value) -> Option<Value> {
    let v = args.get("args")?;
    if v.is_null() {
        return None;
    }
    Some(v.clone())
}

/// Prepend `_G._args = <lua_expr>` to the Lua source when user args are present.
///
/// When `user_args` is `None`, the source is returned unchanged.
pub fn inject_args_globals(lua_source: &str, user_args: Option<&Value>) -> String {
    let Some(args) = user_args else {
        return lua_source.to_string();
    };
    let lua_expr = json_to_lua(args);
    format!("_G._args = {lua_expr}\n{lua_source}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn concurrency_absent_returns_none() {
        assert_eq!(parse_concurrency(&json!({})).unwrap(), None);
    }

    #[test]
    fn concurrency_null_returns_none() {
        assert_eq!(parse_concurrency(&json!({"concurrency": null})).unwrap(), None);
    }

    #[test]
    fn concurrency_valid() {
        assert_eq!(parse_concurrency(&json!({"concurrency": 8})).unwrap(), Some(8));
    }

    #[test]
    fn concurrency_out_of_range() {
        assert!(parse_concurrency(&json!({"concurrency": 0})).is_err());
        assert!(parse_concurrency(&json!({"concurrency": 65})).is_err());
    }

    #[test]
    fn concurrency_non_integer() {
        assert!(parse_concurrency(&json!({"concurrency": "fast"})).is_err());
    }

    #[test]
    fn events_filter_defaults() {
        let f = EventsFilter::from_args(&json!({}));
        assert_eq!(f.offset, 0);
        assert_eq!(f.events_limit, DEFAULT_EVENTS_LIMIT);
        assert_eq!(f.types, None);
        assert_eq!(f.agent_id, None);
    }

    #[test]
    fn events_filter_parsed() {
        let f = EventsFilter::from_args(&json!({
            "offset": 10,
            "events_limit": 5,
            "types": ["agent_started", "agent_done"],
            "agent_id": "abc-123"
        }));
        assert_eq!(f.offset, 10);
        assert_eq!(f.events_limit, 5);
        assert_eq!(f.types.as_ref().unwrap().len(), 2);
        assert_eq!(f.agent_id.as_ref().unwrap(), "abc-123");
    }

    #[test]
    fn events_limit_clamped() {
        assert_eq!(
            parse_events_limit(&json!({"events_limit": 0})),
            1
        );
        assert_eq!(
            parse_events_limit(&json!({"events_limit": 9999})),
            MAX_EVENTS_LIMIT
        );
    }

    #[test]
    fn events_filter_matches() {
        let f = EventsFilter::from_args(&json!({
            "types": ["agent_done"],
            "agent_id": "a1"
        }));
        assert!(f.matches(&json!({"type": "agent_done", "agent_id": "a1"})));
        assert!(!f.matches(&json!({"type": "agent_started", "agent_id": "a1"})));
        assert!(!f.matches(&json!({"type": "agent_done", "agent_id": "a2"})));
    }

    #[test]
    fn paginate_basic() {
        let items: Vec<i32> = (0..10).collect();
        let (page, total, next) = paginate(&items, 2, 3);
        assert_eq!(page, vec![2, 3, 4]);
        assert_eq!(total, 10);
        assert_eq!(next, Some(5));
    }

    #[test]
    fn paginate_last_page() {
        let items: Vec<i32> = (0..5).collect();
        let (page, total, next) = paginate(&items, 3, 10);
        assert_eq!(page, vec![3, 4]);
        assert_eq!(total, 5);
        assert_eq!(next, None);
    }

    #[test]
    fn paginate_cursor_basic() {
        let items = vec!["a", "b", "c", "d", "e"];
        let (page, next) = paginate_cursor(&items, None, 2, |s| *s);
        assert_eq!(page, vec![&"a", &"b"]);
        assert_eq!(next, Some("b".to_string()));

        let (page2, next2) = paginate_cursor(&items, Some("b"), 2, |s| *s);
        assert_eq!(page2, vec![&"c", &"d"]);
        assert_eq!(next2, Some("d".to_string()));

        let (page3, next3) = paginate_cursor(&items, Some("d"), 2, |s| *s);
        assert_eq!(page3, vec![&"e"]);
        assert_eq!(next3, None);
    }

    #[test]
    fn extract_user_args_missing() {
        assert!(extract_user_args(&json!({})).is_none());
    }

    #[test]
    fn extract_user_args_null_is_none() {
        assert!(extract_user_args(&json!({"args": null})).is_none());
    }

    #[test]
    fn extract_user_args_object() {
        let v = extract_user_args(&json!({"args": {"topic": "rust"}})).unwrap();
        assert_eq!(v["topic"], "rust");
    }

    #[test]
    fn inject_no_args_returns_source_unchanged() {
        let src = "function main() end";
        assert_eq!(inject_args_globals(src, None), src);
    }

    #[test]
    fn inject_prepends_global_assignment() {
        let src = "function main() end";
        let out = inject_args_globals(src, Some(&json!({"topic": "rust"})));
        assert!(out.starts_with("_G._args = {topic = \"rust\"}\n"));
        assert!(out.ends_with(src));
    }

    // ── list-query params ────────────────────────────────────────────────

    #[test]
    fn list_limit_default() {
        assert_eq!(parse_list_limit(&json!({})).unwrap(), DEFAULT_LIST_LIMIT);
    }

    #[test]
    fn list_limit_null_is_default() {
        assert_eq!(parse_list_limit(&json!({"limit": null})).unwrap(), DEFAULT_LIST_LIMIT);
    }

    #[test]
    fn list_limit_explicit() {
        assert_eq!(parse_list_limit(&json!({"limit": 5})).unwrap(), 5);
    }

    #[test]
    fn list_limit_at_bounds() {
        assert_eq!(parse_list_limit(&json!({"limit": 1})).unwrap(), 1);
        assert_eq!(parse_list_limit(&json!({"limit": MAX_LIST_LIMIT})).unwrap(), MAX_LIST_LIMIT);
    }

    #[test]
    fn list_limit_rejects_zero() {
        assert!(parse_list_limit(&json!({"limit": 0})).is_err());
    }

    #[test]
    fn list_limit_rejects_over_max() {
        assert!(parse_list_limit(&json!({"limit": MAX_LIST_LIMIT + 1})).is_err());
    }

    #[test]
    fn list_limit_rejects_non_integer() {
        assert!(parse_list_limit(&json!({"limit": "many"})).is_err());
    }

    #[test]
    fn status_filter_absent_is_none() {
        assert_eq!(parse_status_filter(&json!({})).unwrap(), None);
    }

    #[test]
    fn status_filter_null_is_none() {
        assert_eq!(parse_status_filter(&json!({"status_filter": null})).unwrap(), None);
    }

    #[test]
    fn status_filter_case_insensitive() {
        assert_eq!(parse_status_filter(&json!({"status_filter": "COMPLETED"})).unwrap(), Some("completed".into()));
    }

    #[test]
    fn status_filter_rejects_invalid() {
        assert!(parse_status_filter(&json!({"status_filter": "running"})).is_err());
    }

    #[test]
    fn cursor_absent_is_none() {
        assert!(parse_cursor(&json!({})).is_none());
    }

    #[test]
    fn cursor_null_is_none() {
        assert!(parse_cursor(&json!({"cursor": null})).is_none());
    }

    #[test]
    fn cursor_empty_is_none() {
        assert!(parse_cursor(&json!({"cursor": ""})).is_none());
    }

    #[test]
    fn cursor_present() {
        assert_eq!(parse_cursor(&json!({"cursor": "abc"})), Some("abc".into()));
    }
}
