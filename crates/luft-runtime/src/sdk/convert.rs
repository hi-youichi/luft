//! Lua ⇄ JSON conversion helpers shared by the SDK primitives.
//!
//! These are the bridge between the mlua VM and `serde_json::Value`. They are
//! used by every SDK function that passes data across the boundary (agent
//! results, pipeline stage I/O, `json.encode/decode`, the `args` global, …).

use mlua::{Lua, Value};

/// A JSON value that converts into Lua lazily against the target VM.
///
/// Used to pass arguments to a Lua function from a `'static` closure that has
/// no direct `&Lua` handle (e.g. pipeline stage handlers).
pub(crate) struct JsonArg(pub serde_json::Value);

impl mlua::IntoLua for JsonArg {
    fn into_lua(self, lua: &Lua) -> mlua::Result<Value> {
        lua_value_from_json(lua, self.0)
    }
}

/// Convert a Lua value to a `serde_json::Value`.
pub(crate) fn value_to_json(value: Value) -> mlua::Result<serde_json::Value> {
    match value {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(b) => Ok(serde_json::Value::Bool(b)),
        Value::LightUserData(_) => Ok(serde_json::Value::Null),
        Value::Integer(i) => Ok(serde_json::Value::Number(i.into())),
        Value::Number(n) => Ok(serde_json::json!(n)),
        Value::String(s) => Ok(serde_json::Value::String(
            s.to_str().map(|s| s.to_string()).unwrap_or_default(),
        )),
        Value::Table(t) => {
            // Distinguish array-like from map-like tables.
            let len = t.raw_len();
            if len > 0 {
                let mut arr = Vec::with_capacity(len);
                for i in 1..=len {
                    arr.push(value_to_json(t.get(i)?)?);
                }
                Ok(serde_json::Value::Array(arr))
            } else {
                let mut map = serde_json::Map::new();
                for pair in t.pairs::<Value, Value>() {
                    let (k, v) = pair?;
                    let key = match k {
                        Value::String(s) => s.to_str().map(|s| s.to_string()).unwrap_or_default(),
                        Value::Integer(i) => i.to_string(),
                        Value::Number(n) => n.to_string(),
                        _ => continue,
                    };
                    map.insert(key, value_to_json(v)?);
                }
                Ok(serde_json::Value::Object(map))
            }
        }
        Value::Function(_) => Ok(serde_json::Value::Null),
        Value::Thread(_) => Ok(serde_json::Value::Null),
        Value::UserData(_) => Ok(serde_json::Value::Null),
        Value::Error(e) => Err(mlua::Error::RuntimeError(format!("lua error: {}", e))),
        _ => Ok(serde_json::Value::Null),
    }
}

/// Convert a JSON string to a Lua value.
pub(crate) fn json_string_to_value(lua: &Lua, s: &str) -> mlua::Result<Value> {
    let json: serde_json::Value = serde_json::from_str(s)
        .map_err(|e| mlua::Error::RuntimeError(format!("json decode error: {}", e)))?;
    lua_value_from_json(lua, json)
}

/// Convert a `serde_json::Value` to a Lua value.
pub(crate) fn lua_value_from_json(lua: &Lua, json: serde_json::Value) -> mlua::Result<Value> {
    match json {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else {
                Ok(Value::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(&s)?)),
        serde_json::Value::Array(arr) => {
            let t = lua.create_table()?;
            for (i, v) in arr.into_iter().enumerate() {
                t.set(i + 1, lua_value_from_json(lua, v)?)?;
            }
            Ok(Value::Table(t))
        }
        serde_json::Value::Object(map) => {
            let t = lua.create_table()?;
            for (k, v) in map {
                t.set(k, lua_value_from_json(lua, v)?)?;
            }
            Ok(Value::Table(t))
        }
    }
}

/// Convert a `serde_json::Value` to a Lua table (for the `args` global).
pub(crate) fn serde_json_to_lua(lua: &Lua, json: serde_json::Value) -> mlua::Result<mlua::Table> {
    match lua_value_from_json(lua, json)? {
        Value::Table(t) => Ok(t),
        _ => Ok(lua.create_table()?),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mlua::Lua;

    #[test]
    fn primitives_lua_to_json() {
        let lua = Lua::new();
        assert_eq!(value_to_json(Value::Nil).unwrap(), serde_json::Value::Null);
        assert_eq!(
            value_to_json(Value::Boolean(true)).unwrap(),
            serde_json::json!(true)
        );
        assert_eq!(
            value_to_json(Value::Integer(42)).unwrap(),
            serde_json::json!(42)
        );
        let s = Value::String(lua.create_string("hi").unwrap());
        assert_eq!(value_to_json(s).unwrap(), serde_json::json!("hi"));
    }

    #[test]
    fn table_with_sequence_is_array() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.set(1, 10).unwrap();
        t.set(2, 20).unwrap();
        assert_eq!(
            value_to_json(Value::Table(t)).unwrap(),
            serde_json::json!([10, 20])
        );
    }

    #[test]
    fn table_with_string_keys_is_object() {
        let lua = Lua::new();
        let t = lua.create_table().unwrap();
        t.set("name", "x").unwrap();
        assert_eq!(
            value_to_json(Value::Table(t)).unwrap(),
            serde_json::json!({ "name": "x" })
        );
    }

    #[test]
    fn json_lua_json_round_trip() {
        let lua = Lua::new();
        let original = serde_json::json!({
            "name": "test",
            "count": 3,
            "ratio": 1.5,
            "tags": ["a", "b"],
            "nested": { "ok": true }
        });
        let lua_val = lua_value_from_json(&lua, original.clone()).unwrap();
        assert_eq!(value_to_json(lua_val).unwrap(), original);
    }

    #[test]
    fn empty_array_round_trips_to_object() {
        // Lua tables can't distinguish an empty array from an empty map, so an
        // empty JSON array becomes an empty object. Locks in known behavior.
        let lua = Lua::new();
        let lua_val = lua_value_from_json(&lua, serde_json::json!([])).unwrap();
        assert_eq!(value_to_json(lua_val).unwrap(), serde_json::json!({}));
    }

    #[test]
    fn json_decode_parses_and_rejects() {
        let lua = Lua::new();
        let v = json_string_to_value(&lua, r#"{"a":1}"#).unwrap();
        assert_eq!(value_to_json(v).unwrap(), serde_json::json!({ "a": 1 }));
        assert!(json_string_to_value(&lua, "{not json").is_err());
    }

    #[test]
    fn serde_json_to_lua_scalar_falls_back_to_empty_table() {
        let lua = Lua::new();
        // Non-container JSON yields an empty table (the `args` global fallback).
        let scalar = serde_json_to_lua(&lua, serde_json::json!(42)).unwrap();
        assert_eq!(scalar.raw_len(), 0);
        let obj = serde_json_to_lua(&lua, serde_json::json!({ "k": 1 })).unwrap();
        assert_eq!(obj.get::<i64>("k").unwrap(), 1);
    }
}
