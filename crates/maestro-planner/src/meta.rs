//! `planner::meta` — Declarative workflow metadata extraction.
//!
//! Extracts the `meta = { phases = {...}, reasoning = "..." }` table from a
//! Lua workflow script without running `main()`.

use maestro_runtime::ScriptError;
use mlua::{Lua, Table, Value};
use serde::{Deserialize, Serialize};

/// Declarative phase description for progress display and tracking.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PlanMeta {
    pub phases: Vec<MetaPhase>,
    #[serde(default)]
    pub reasoning: String,
}

/// Single phase in a workflow plan. `depends_on` uses 1-based indices.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct MetaPhase {
    pub label: String,
    pub detail: String,
    #[serde(default)]
    pub agents: usize,
    #[serde(default)]
    pub depends_on: Vec<u32>,
}

/// Result of post-extraction validation.
#[derive(Debug, Clone, Default)]
pub struct MetaValidation {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl MetaValidation {
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Extract the `meta` table from a Lua workflow script.
///
/// Returns `None` if the script has no `meta` global or if it's not a table.
/// Extraction is side-effect-free: `main()` is never called.
pub fn extract_meta(script: &str) -> Result<Option<PlanMeta>, ScriptError> {
    let lua = Lua::new();
    register_stubs(&lua).map_err(ScriptError::from)?;
    lua.load(script)
        .exec()
        .map_err(|e| ScriptError::Internal(format!("top-level exec failed: {e}")))?;

    let globals = lua.globals();
    let value: Value = match globals.get("meta") {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    if !matches!(value, Value::Table(_)) {
        return Ok(None);
    }

    let meta_table: Table = value.as_table().cloned().expect("checked Value::Table");

    let phases_value: Table = match meta_table.get("phases") {
        Ok(t) => t,
        Err(_) => return Ok(None),
    };

    let mut phases = Vec::new();
    let len = phases_value.raw_len();
    for i in 1..=len {
        let v: Value = phases_value
            .raw_get(i)
            .map_err(|e| ScriptError::Internal(format!("meta.phases[{i}]: {e}")))?;
        let phase = lua_to_meta_phase(v)?;
        phases.push(phase);
    }

    let reasoning: String = meta_table
        .get::<Option<String>>("reasoning")
        .map_err(|e| ScriptError::Internal(format!("meta.reasoning: {e}")))?
        .unwrap_or_default();

    Ok(Some(PlanMeta { phases, reasoning }))
}

fn lua_to_meta_phase(value: Value) -> Result<MetaPhase, ScriptError> {
    let table = match value {
        Value::Table(t) => t,
        _ => return Err(ScriptError::Internal("meta.phases[*] must be a table".into())),
    };
    let label: String = table
        .get("label")
        .map_err(|e| ScriptError::Internal(format!("meta.phases[*].label: {e}")))?;
    let detail: String = table
        .get("detail")
        .map_err(|e| ScriptError::Internal(format!("meta.phases[*].detail: {e}")))?;
    let agents: usize = table
        .get::<Option<usize>>("agents")
        .map_err(|e| ScriptError::Internal(format!("meta.phases[*].agents: {e}")))?
        .unwrap_or(0);
    let depends_on: Vec<u32> = table
        .get::<Option<Vec<u32>>>("depends_on")
        .map_err(|e| ScriptError::Internal(format!("meta.phases[*].depends_on: {e}")))?
        .unwrap_or_default();
    Ok(MetaPhase { label, detail, agents, depends_on })
}

/// Register no-op stubs so accidental top-level SDK calls don't crash extraction.
fn register_stubs(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    let phase = lua.create_function(|_, _: mlua::MultiValue| Ok(0u32))?;
    globals.set("phase", phase)?;
    let report = lua.create_function(|_, _: mlua::MultiValue| Ok(Value::Nil))?;
    globals.set("report", report)?;
    let log = lua.create_function(|_, _: mlua::MultiValue| Ok(Value::Nil))?;
    globals.set("log", log)?;
    let budget = lua.create_function(|_, _: mlua::MultiValue| Ok(Value::Nil))?;
    globals.set("budget", budget)?;
    let agent = lua.create_function(|lua, _: mlua::MultiValue| Ok(Value::Table(lua.create_table()?)))?;
    globals.set("agent", agent)?;
    let parallel = lua.create_function(|lua, _: mlua::MultiValue| Ok(Value::Table(lua.create_table()?)))?;
    globals.set("parallel", parallel)?;
    let pipeline = lua.create_function(|lua, _: mlua::MultiValue| Ok(Value::Table(lua.create_table()?)))?;
    globals.set("pipeline", pipeline)?;
    let workflow = lua.create_function(|lua, _: mlua::MultiValue| Ok(Value::Table(lua.create_table()?)))?;
    globals.set("workflow", workflow)?;

    let json_table = lua.create_table()?;
    let encode = lua.create_function(|_, v: Value| {
        Ok(serde_json::to_string(&lua_value_to_json(&v)).unwrap_or_default())
    })?;
    let decode = lua.create_function(|lua, s: String| match serde_json::from_str::<serde_json::Value>(&s) {
        Ok(v) => json_to_lua_value(lua, &v),
        Err(_) => Ok(Value::Nil),
    })?;
    json_table.set("encode", encode)?;
    json_table.set("decode", decode)?;
    globals.set("json", json_table)?;

    globals.set("args", lua.create_table()?)?;
    let ctx = lua.create_table()?;
    ctx.set("run_id", "")?;
    globals.set("ctx", ctx)?;
    Ok(())
}

fn lua_value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Integer(i) => serde_json::json!(i),
        Value::Number(n) => serde_json::json!(n),
        Value::String(s) => serde_json::Value::String(s.to_string_lossy()),
        Value::Table(t) => {
            let len = t.raw_len();
            if len > 0 {
                let mut arr = Vec::with_capacity(len);
                for i in 1..=len {
                    match t.raw_get::<Value>(i) {
                        Ok(v) => arr.push(lua_value_to_json(&v)),
                        Err(_) => break,
                    }
                }
                if arr.len() == len {
                    return serde_json::Value::Array(arr);
                }
            }
            let mut map = serde_json::Map::new();
            for pair in t.pairs::<Value, Value>().flatten() {
                if let Value::String(k) = pair.0 {
                    map.insert(k.to_string_lossy(), lua_value_to_json(&pair.1));
                }
            }
            serde_json::Value::Object(map)
        }
        _ => serde_json::Value::Null,
    }
}

fn json_to_lua_value(lua: &Lua, v: &serde_json::Value) -> mlua::Result<Value> {
    match v {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() { Ok(Value::Integer(i)) }
            else if let Some(f) = n.as_f64() { Ok(Value::Number(f)) }
            else { Ok(Value::Nil) }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(s)?)),
        serde_json::Value::Array(arr) => {
            let t = lua.create_table()?;
            for (i, item) in arr.iter().enumerate() {
                t.raw_set(i + 1, json_to_lua_value(lua, item)?)?;
            }
            Ok(Value::Table(t))
        }
        serde_json::Value::Object(map) => {
            let t = lua.create_table()?;
            for (k, val) in map {
                t.set(k.as_str(), json_to_lua_value(lua, val)?)?;
            }
            Ok(Value::Table(t))
        }
    }
}

/// Validate the extracted meta.
pub fn validate_meta(meta: &PlanMeta, script: &str) -> MetaValidation {
    let mut out = MetaValidation::default();

    if meta.phases.is_empty() {
        out.warnings.push("meta.phases is empty; progress display will show nothing".into());
    }

    let mut seen_labels = std::collections::HashSet::new();
    for (idx, phase) in meta.phases.iter().enumerate() {
        if phase.label.trim().is_empty() {
            out.errors.push(format!("meta.phases[{}].label is empty", idx));
        }
        if !seen_labels.insert(phase.label.as_str()) {
            out.errors.push(format!("duplicate phase label: '{}'", phase.label));
        }
        for dep in &phase.depends_on {
            if *dep == 0 {
                out.errors.push(format!("meta.phases[{}].depends_on uses 0; must be 1-based", idx));
                continue;
            }
            let dep_idx = (*dep as usize).saturating_sub(1);
            if dep_idx >= meta.phases.len() {
                out.errors.push(format!(
                    "meta.phases[{}].depends_on={} out of range (have {} phases)",
                    idx, dep, meta.phases.len()
                ));
            } else if dep_idx == idx {
                out.errors.push(format!("meta.phases[{}] depends on itself", idx));
            }
        }
    }

    if !meta.phases.is_empty() {
        for phase in &meta.phases {
            let needle1 = format!("phase(\"{}\"", phase.label);
            let needle2 = format!("phase('{}'", phase.label);
            if !script.contains(&needle1) && !script.contains(&needle2) {
                out.warnings.push(format!(
                    "phase label '{}' has no matching phase(...) call in script",
                    phase.label
                ));
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_meta_full() {
        let script = r#"
meta = {
    phases = {
        { label = "discovery", detail = "find files", agents = 1, depends_on = {} },
        { label = "analysis", detail = "analyze", agents = 3, depends_on = { 1 } }
    },
    reasoning = "two-stage pipeline"
}
function main() report({ ok = true }) end
"#;
        let meta = extract_meta(script).unwrap().unwrap();
        assert_eq!(meta.phases.len(), 2);
        assert_eq!(meta.phases[0].label, "discovery");
        assert_eq!(meta.phases[1].depends_on, vec![1]);
        assert_eq!(meta.reasoning, "two-stage pipeline");
    }

    #[test]
    fn extract_meta_missing() {
        assert!(extract_meta("report({ ok = true })").unwrap().is_none());
    }

    #[test]
    fn extract_meta_not_a_table() {
        assert!(extract_meta("meta = \"hello\"\nreport({})").unwrap().is_none());
    }

    #[test]
    fn extract_meta_default_values() {
        let script = "meta = { phases = { { label = 'p', detail = 'd' } } }\nfunction main() report({}) end";
        let meta = extract_meta(script).unwrap().unwrap();
        assert_eq!(meta.phases[0].agents, 0);
        assert!(meta.phases[0].depends_on.is_empty());
    }

    #[test]
    fn extract_meta_top_level_phase_safe() {
        let script = "meta = { phases = { { label = 'x', detail = 'y' } } }\nphase('x', 1)\nfunction main() report({}) end";
        let meta = extract_meta(script).unwrap().unwrap();
        assert_eq!(meta.phases[0].label, "x");
    }

    #[test]
    fn validate_meta_ok() {
        let meta = PlanMeta {
            phases: vec![
                MetaPhase { label: "a".into(), detail: "1".into(), agents: 1, depends_on: vec![] },
                MetaPhase { label: "b".into(), detail: "2".into(), agents: 2, depends_on: vec![1] },
            ],
            reasoning: String::new(),
        };
        let v = validate_meta(&meta, "phase(\"a\", 1); phase(\"b\", 2)");
        assert!(v.is_valid());
    }

    #[test]
    fn validate_meta_duplicate_label() {
        let meta = PlanMeta {
            phases: vec![
                MetaPhase { label: "a".into(), detail: "1".into(), ..Default::default() },
                MetaPhase { label: "a".into(), detail: "2".into(), ..Default::default() },
            ],
            reasoning: String::new(),
        };
        assert!(!validate_meta(&meta, "").is_valid());
    }

    #[test]
    fn validate_meta_zero_depends_on() {
        let meta = PlanMeta {
            phases: vec![
                MetaPhase { label: "a".into(), detail: "1".into(), ..Default::default() },
                MetaPhase { label: "b".into(), detail: "2".into(), depends_on: vec![0], ..Default::default() },
            ],
            reasoning: String::new(),
        };
        let v = validate_meta(&meta, "");
        assert!(v.errors.iter().any(|e| e.contains("1-based")));
    }

    #[test]
    fn validate_meta_self_dependency() {
        let meta = PlanMeta {
            phases: vec![MetaPhase { label: "a".into(), detail: "1".into(), depends_on: vec![1], ..Default::default() }],
            reasoning: String::new(),
        };
        assert!(!validate_meta(&meta, "").is_valid());
    }

    #[test]
    fn validate_meta_out_of_range() {
        let meta = PlanMeta {
            phases: vec![MetaPhase { label: "a".into(), detail: "1".into(), depends_on: vec![5], ..Default::default() }],
            reasoning: String::new(),
        };
        assert!(!validate_meta(&meta, "").is_valid());
    }

    #[test]
    fn plan_meta_serde_roundtrip() {
        let meta = PlanMeta {
            phases: vec![MetaPhase { label: "x".into(), detail: "y".into(), agents: 3, depends_on: vec![1, 2] }],
            reasoning: "why".into(),
        };
        let json = serde_json::to_string(&meta).unwrap();
        assert_eq!(meta, serde_json::from_str(&json).unwrap());
    }
}
