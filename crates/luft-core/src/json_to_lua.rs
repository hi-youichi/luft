//! Encode `serde_json::Value` as a Lua expression.
//!
//! Used by workflow tool consumers to inject user-supplied `args` into the Lua
//! global `_G._args` so that workflows can read it as a regular Lua table.
//!
//! Limitations:
//! - JSON `null` becomes Lua `nil`.
//! - JSON numbers are emitted as Rust `i64`/`u64` when representable, otherwise
//!   as the default float formatting. Lossy beyond ~15 significant digits.
//! - JSON object keys that are not valid Lua identifiers are emitted as
//!   `["key"]` string-indexed entries.
//! - String contents are escaped for double-quoted Lua string literals;
//!   control characters below 0x20 are emitted as `\xHH`.

use serde_json::Value;
use std::fmt::Write;

pub fn json_to_lua(value: &Value) -> String {
    let mut out = String::new();
    write_value(value, &mut out);
    out
}

fn write_value(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("nil"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                let _ = write!(out, "{i}");
            } else if let Some(u) = n.as_u64() {
                let _ = write!(out, "{u}");
            } else {
                let _ = write!(out, "{n}");
            }
        }
        Value::String(s) => write_string(s, out),
        Value::Array(items) => {
            out.push('{');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value(item, out);
            }
            out.push('}');
        }
        Value::Object(map) => {
            out.push('{');
            let mut first = true;
            for (k, v) in map {
                if !first {
                    out.push_str(", ");
                }
                first = false;
                write_object_key(k, out);
                out.push_str(" = ");
                write_value(v, out);
            }
            out.push('}');
        }
    }
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            '\x07' => out.push_str("\\a"),
            '\x0b' => out.push_str("\\v"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\x{:02x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_object_key(k: &str, out: &mut String) {
    if is_valid_lua_ident(k) {
        out.push_str(k);
    } else {
        write_string(k, out);
    }
}

fn is_valid_lua_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn primitives() {
        assert_eq!(json_to_lua(&json!(null)), "nil");
        assert_eq!(json_to_lua(&json!(true)), "true");
        assert_eq!(json_to_lua(&json!(false)), "false");
        assert_eq!(json_to_lua(&json!(42)), "42");
        assert_eq!(json_to_lua(&json!(-7)), "-7");
        assert_eq!(json_to_lua(&json!(0)), "0");
    }

    #[test]
    fn floats_emit_as_decimal() {
        assert_eq!(json_to_lua(&json!(3.5)), "3.5");
    }

    #[test]
    fn large_unsigned() {
        assert_eq!(json_to_lua(&json!(9_000_000_000_u64)), "9000000000");
    }

    #[test]
    fn empty_collections() {
        assert_eq!(json_to_lua(&json!([])), "{}");
        assert_eq!(json_to_lua(&json!({})), "{}");
    }

    #[test]
    fn array_of_ints() {
        assert_eq!(json_to_lua(&json!([1, 2, 3])), "{1, 2, 3}");
    }

    #[test]
    fn mixed_array() {
        assert_eq!(
            json_to_lua(&json!(["a", 1, true, null])),
            "{\"a\", 1, true, nil}"
        );
    }

    #[test]
    fn object_simple_keys() {
        assert_eq!(
            json_to_lua(&json!({"topic": "rust", "n": 10})),
            "{n = 10, topic = \"rust\"}"
        );
    }

    #[test]
    fn object_invalid_ident_keys_quoted() {
        assert_eq!(
            json_to_lua(&json!({"with-dash": 1, "9starts_with_digit": 2})),
            "{\"9starts_with_digit\" = 2, \"with-dash\" = 1}"
        );
    }

    #[test]
    fn string_escaping() {
        assert_eq!(
            json_to_lua(&json!("hello \"world\"\n\t\\end")),
            "\"hello \\\"world\\\"\\n\\t\\\\end\""
        );
    }

    #[test]
    fn string_with_control_char() {
        assert_eq!(json_to_lua(&json!("\x01ok")), "\"\\x01ok\"");
    }

    #[test]
    fn nested_structure() {
        assert_eq!(
            json_to_lua(&json!({
                "topic": "rust",
                "tags": ["async", "tokio"],
                "opts": {"depth": 2, "recursive": false}
            })),
            "{opts = {depth = 2, recursive = false}, tags = {\"async\", \"tokio\"}, topic = \"rust\"}"
        );
    }

    #[test]
    fn lua_ident_edge_cases() {
        assert!(is_valid_lua_ident("_x"));
        assert!(is_valid_lua_ident("x9"));
        assert!(!is_valid_lua_ident("9x"));
        assert!(!is_valid_lua_ident("x-y"));
        assert!(!is_valid_lua_ident(""));
        assert!(!is_valid_lua_ident("with space"));
    }

    #[test]
    fn roundtrip_object_count() {
        // The Lua table should preserve the number of fields
        let v = json!({"a": 1, "b": 2, "c": 3});
        let lua = json_to_lua(&v);
        assert_eq!(lua.matches(" = ").count(), 3);
    }
}
