//! `lua validate` — validate a Lua workflow script without executing it.
//!
//! Performs three layers of checks:
//! 1. Syntax — compiles the script and executes top-level (safe: only `meta`
//!    assignment and function definitions run, `main()` is never called).
//! 2. Structure — verifies `meta` table and `main()` function exist.
//! 3. Heuristic — checks for `report(...)` call and `phase_begin/phase_end`
//!    pairing.

use anyhow::Result;
use clap::Subcommand;
use std::path::PathBuf;

use maestro::runtime::{validate_workflow, WorkflowValidation};

#[derive(Debug, Subcommand)]
pub enum LuaSubcommand {
    /// Validate a Lua workflow script without executing it.
    Validate(LuaValidateArgs),
    /// Check that a .mock.json sidecar covers all named agent calls in a script.
    MockCheck(MockCheckArgs),
}

#[derive(Debug, clap::Args)]
pub struct LuaValidateArgs {
    /// Path to the .lua file to validate.
    pub file: PathBuf,

    /// Verbose output (show extracted meta details).
    #[arg(short, long)]
    pub verbose: bool,
}

/// Run the `lua validate` subcommand.
pub fn validate_lua(args: LuaValidateArgs) -> Result<()> {
    let file = &args.file;
    let script = std::fs::read_to_string(file)
        .map_err(|e| anyhow::anyhow!("cannot read '{}': {}", file.display(), e))?;

    let display = file.display().to_string();

    match validate_workflow(&script) {
        Err(e) => {
            eprintln!("! {} - script error:", display);
            eprintln!("    {}", e);
            anyhow::bail!("validation failed");
        }
        Ok(result) => {
            if args.verbose {
                print_verbose(&display, &result);
            } else {
                print_compact(&display, &result);
            }
            if !result.is_valid() {
                anyhow::bail!("validation failed");
            }
            Ok(())
        }
    }
}

fn print_compact(file: &str, result: &WorkflowValidation) {
    if result.is_valid() {
        println!("ok {} - valid", file);
    } else {
        print_errors(file, &result.errors, false);
    }
}

fn print_verbose(file: &str, result: &WorkflowValidation) {
    println!("Syntax: ok\n");

    match &result.meta {
        Some(meta) => {
            let phase_labels: Vec<&str> = meta.phases.iter().map(|p| p.label.as_str()).collect();
            println!(
                "Meta: {} phase(s): {}",
                meta.phases.len(),
                phase_labels.join(", ")
            );
            if !meta.reasoning.is_empty() {
                println!("  reasoning: \"{}\"", meta.reasoning);
            }
            for (i, phase) in meta.phases.iter().enumerate() {
                let kind = if phase.dynamic { "dynamic" } else { "static" };
                println!("  {}. {} ({})", i + 1, phase.label, kind);
            }
        }
        None => {
            println!("Meta: missing");
        }
    }

    println!("\nmain():    {}", presence_label(result.has_main));
    println!("report():  {}", presence_label(result.has_report_call));
    println!(
        "phase_begin/end paired: {}",
        presence_label(result.span_pairing_ok)
    );

    if !result.warnings.is_empty() {
        println!("\nWarnings:");
        for w in &result.warnings {
            println!("  - {}", w);
        }
    }

    if result.is_valid() {
        println!("\nok {} - valid", file);
    } else {
        print_errors(file, &result.errors, true);
    }
}

fn print_errors(file: &str, errors: &[String], leading_newline: bool) {
    if leading_newline {
        eprintln!();
    }
    eprintln!("FAIL {} - {} error(s):", file, errors.len());
    for err in errors {
        eprintln!("  - {}", err);
    }
}

fn presence_label(ok: bool) -> &'static str {
    if ok {
        "found"
    } else {
        "missing"
    }
}

// ── mock-check ─────────────────────────────────────────────────────

#[derive(Debug, clap::Args)]
pub struct MockCheckArgs {
    /// Path to the .lua workflow script.
    pub script: PathBuf,

    /// Path to the .mock.json file (default: <script>.mock.json).
    #[arg(long)]
    pub mock: Option<PathBuf>,
}

/// Run `lua mock-check`: statically verify .mock.json coverage.
pub fn mock_check(args: MockCheckArgs) -> Result<()> {
    let script = std::fs::read_to_string(&args.script)
        .map_err(|e| anyhow::anyhow!("cannot read '{}': {}", args.script.display(), e))?;

    let mock_path = args
        .mock
        .clone()
        .unwrap_or_else(|| args.script.with_extension("mock.json"));

    if !mock_path.exists() {
        anyhow::bail!(
            "mock file not found: {}. Run `maestro generate --with-mock` first.",
            mock_path.display()
        );
    }

    let mock_content = std::fs::read_to_string(&mock_path)
        .map_err(|e| anyhow::anyhow!("cannot read '{}': {}", mock_path.display(), e))?;

    let mock_file: serde_json::Value = serde_json::from_str(&mock_content).map_err(|e| {
        anyhow::anyhow!(
            "invalid mock JSON: {}. Expected format: {{\"responses\": {{...}}}}",
            e
        )
    })?;

    let mock_responses = mock_file
        .get("responses")
        .and_then(|r| r.as_object())
        .cloned()
        .unwrap_or_default();

    let has_default = mock_file.get("default").is_some();

    let lua_names = maestro::mock_gen::extract_agent_names(&script);
    let (matched, missing, extra, coverage_pct) =
        compute_coverage(&lua_names, &mock_responses);

    print_coverage_report(
        args.script.as_path(),
        mock_path.as_path(),
        &lua_names,
        &mock_responses,
        matched.len(),
        &missing,
        &extra,
        coverage_pct,
        has_default,
    );

    if missing.is_empty() {
        println!("  Status:   OK");
        Ok(())
    } else if has_default {
        println!("  Status:   OK (with default fallback for unmatched)");
        Ok(())
    } else {
        println!("  Status:   FAIL");
        anyhow::bail!(
            "mock coverage incomplete: {} of {} agent names have no mock response",
            missing.len(),
            lua_names.len()
        );
    }
}

fn compute_coverage<'a>(
    lua_names: &'a [String],
    mock_responses: &'a serde_json::Map<String, serde_json::Value>,
) -> (Vec<&'a String>, Vec<&'a String>, Vec<&'a String>, f64) {
    use std::collections::HashSet;

    let lua_names_set: HashSet<&str> = lua_names.iter().map(|s| s.as_str()).collect();
    let mock_names_set: HashSet<&str> = mock_responses.keys().map(|s| s.as_str()).collect();

    let matched: Vec<&String> = lua_names
        .iter()
        .filter(|n| mock_names_set.contains(n.as_str()))
        .collect();
    let missing: Vec<&String> = lua_names
        .iter()
        .filter(|n| !mock_names_set.contains(n.as_str()))
        .collect();
    let extra: Vec<&String> = mock_responses
        .keys()
        .filter(|n| !lua_names_set.contains(n.as_str()))
        .collect();

    let coverage_pct = if lua_names.is_empty() {
        100.0
    } else {
        matched.len() as f64 / lua_names.len() as f64 * 100.0
    };

    (matched, missing, extra, coverage_pct)
}

#[allow(clippy::too_many_arguments)]
fn print_coverage_report(
    script_path: &std::path::Path,
    mock_path: &std::path::Path,
    lua_names: &[String],
    mock_responses: &serde_json::Map<String, serde_json::Value>,
    matched_count: usize,
    missing: &[&String],
    extra: &[&String],
    coverage_pct: f64,
    has_default: bool,
) {
    println!("── Mock Coverage ──");
    println!("  Script:     {}", script_path.display());
    println!("  Mock file:  {}", mock_path.display());
    println!();
    println!("  Agent names in script: {}", lua_names.len());
    println!("  Mock responses:        {}", mock_responses.len());
    println!("  Matched:               {}", matched_count);
    println!("  Missing (no mock):     {}", missing.len());
    println!("  Extra (unused mock):   {}", extra.len());
    if has_default {
        println!("  Default fallback:      yes");
    } else {
        println!("  Default fallback:      no");
    }

    if !missing.is_empty() {
        println!();
        println!("  Missing names:");
        for n in missing {
            println!("    - {}", n);
        }
    }
    if !extra.is_empty() {
        println!();
        println!("  Extra (unused) mock entries:");
        for n in extra {
            println!("    - {}", n);
        }
    }

    println!();
    println!("  Coverage: {:.0}%", coverage_pct);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::{NamedTempFile, TempDir};

    fn write_tmp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
    }

    fn build_script(agent_names: &[&str]) -> String {
        let mut s = String::from(
            r#"
                meta = { reasoning = "test workflow", phases = {} }
                function main()
"#,
        );
        for name in agent_names {
            s.push_str(&format!(
                "                    local r = agent({{ name = \"{}\", prompt = \"p\" }})\n",
                name
            ));
        }
        s.push_str("                    report(r)\n                end\n");
        s
    }

    fn build_mock(mock_responses: &[&str], with_default: bool) -> String {
        let mut m = String::from("{\n  \"responses\": {\n");
        let entries: Vec<String> = mock_responses
            .iter()
            .map(|n| format!("    \"{}\": {{ \"text\": \"x\" }}", n))
            .collect();
        m.push_str(&entries.join(",\n"));
        m.push_str("\n  }");
        if with_default {
            m.push_str(",\n  \"default\": { \"text\": \"default\" }");
        }
        m.push_str("\n}\n");
        m
    }

    const VALID_SCRIPT: &str = r#"
        meta = {
            reasoning = "test workflow",
            phases = {
                { label = "phase1", dynamic = false },
                { label = "phase2", dynamic = true },
            }
        }
        function main()
            local r = agent({ prompt = "do work" })
            report(r)
        end
    "#;

    #[test]
    fn valid_script_passes() {
        let f = write_tmp(VALID_SCRIPT);
        let args = LuaValidateArgs {
            file: f.path().to_path_buf(),
            verbose: false,
        };
        assert!(validate_lua(args).is_ok());
    }

    #[test]
    fn syntax_error_fails() {
        let f = write_tmp("if true then");
        let args = LuaValidateArgs {
            file: f.path().to_path_buf(),
            verbose: false,
        };
        assert!(validate_lua(args).is_err());
    }

    #[test]
    fn missing_main_fails() {
        let f =
            write_tmp("meta = { reasoning = \"x\", phases = {} }\nfunction run() report(1) end");
        let args = LuaValidateArgs {
            file: f.path().to_path_buf(),
            verbose: false,
        };
        assert!(validate_lua(args).is_err());
    }

    #[test]
    fn missing_meta_fails() {
        let f = write_tmp("function main() report(1) end");
        let args = LuaValidateArgs {
            file: f.path().to_path_buf(),
            verbose: false,
        };
        assert!(validate_lua(args).is_err());
    }

    #[test]
    fn missing_report_fails() {
        let f =
            write_tmp("meta = { reasoning = \"x\", phases = {} }\nfunction main() local x = 1 end");
        let args = LuaValidateArgs {
            file: f.path().to_path_buf(),
            verbose: false,
        };
        assert!(validate_lua(args).is_err());
    }

    #[test]
    fn non_existent_file_fails() {
        let args = LuaValidateArgs {
            file: PathBuf::from("/__nonexistent_maestro_test__.lua"),
            verbose: false,
        };
        assert!(validate_lua(args).is_err());
    }

    #[test]
    fn verbose_mode_does_not_panic() {
        let f = write_tmp(VALID_SCRIPT);
        let args = LuaValidateArgs {
            file: f.path().to_path_buf(),
            verbose: true,
        };
        assert!(validate_lua(args).is_ok());
    }

    #[test]
    fn presence_label_true_is_found() {
        assert_eq!(presence_label(true), "found");
    }

    #[test]
    fn presence_label_false_is_missing() {
        assert_eq!(presence_label(false), "missing");
    }

    #[test]
    fn compute_coverage_full_match_is_ok() {
        let names = vec!["a".to_string(), "b".to_string()];
        let mut mock = serde_json::Map::new();
        mock.insert("a".to_string(), serde_json::json!({"text": "x"}));
        mock.insert("b".to_string(), serde_json::json!({"text": "y"}));

        let (matched, missing, extra, coverage) = compute_coverage(&names, &mock);
        assert_eq!(matched.len(), 2);
        assert!(missing.is_empty());
        assert!(extra.is_empty());
        assert!((coverage - 100.0).abs() < 1e-9);
    }

    #[test]
    fn compute_coverage_handles_missing_and_extra() {
        let names = vec!["a".to_string(), "b".to_string()];
        let mut mock = serde_json::Map::new();
        mock.insert("a".to_string(), serde_json::json!({"text": "x"}));
        mock.insert("c".to_string(), serde_json::json!({"text": "z"}));

        let (matched, missing, extra, coverage) = compute_coverage(&names, &mock);
        assert_eq!(matched.len(), 1);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0], "b");
        assert_eq!(extra.len(), 1);
        assert_eq!(extra[0], "c");
        assert!((coverage - 50.0).abs() < 1e-9);
    }

    #[test]
    fn compute_coverage_empty_script_is_full_coverage() {
        let names: Vec<String> = vec![];
        let mut mock = serde_json::Map::new();
        mock.insert("a".to_string(), serde_json::json!({"text": "x"}));

        let (matched, missing, extra, coverage) = compute_coverage(&names, &mock);
        assert!(matched.is_empty());
        assert!(missing.is_empty());
        assert_eq!(extra.len(), 1);
        assert_eq!(extra[0], "a");
        assert!((coverage - 100.0).abs() < 1e-9);
    }

    #[test]
    fn mock_check_full_coverage_ok() {
        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("wf.lua");
        let mock_path = dir.path().join("wf.lua.mock.json");
        std::fs::write(&script_path, build_script(&["a", "b"])).unwrap();
        std::fs::write(&mock_path, build_mock(&["a", "b"], false)).unwrap();

        let args = MockCheckArgs {
            script: script_path,
            mock: Some(mock_path),
        };
        assert!(mock_check(args).is_ok());
    }

    #[test]
    fn mock_check_missing_with_default_fallback_ok() {
        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("wf.lua");
        let mock_path = dir.path().join("wf.lua.mock.json");
        std::fs::write(&script_path, build_script(&["a", "b"])).unwrap();
        std::fs::write(&mock_path, build_mock(&["a"], true)).unwrap();

        let args = MockCheckArgs {
            script: script_path,
            mock: Some(mock_path),
        };
        assert!(mock_check(args).is_ok());
    }

    #[test]
    fn mock_check_missing_without_default_fails() {
        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("wf.lua");
        let mock_path = dir.path().join("wf.lua.mock.json");
        std::fs::write(&script_path, build_script(&["a", "b"])).unwrap();
        std::fs::write(&mock_path, build_mock(&["a"], false)).unwrap();

        let args = MockCheckArgs {
            script: script_path,
            mock: Some(mock_path),
        };
        assert!(mock_check(args).is_err());
    }

    #[test]
    fn mock_check_handles_extra_entries() {
        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("wf.lua");
        let mock_path = dir.path().join("wf.lua.mock.json");
        std::fs::write(&script_path, build_script(&["a"])).unwrap();
        std::fs::write(&mock_path, build_mock(&["a", "unused"], false)).unwrap();

        let args = MockCheckArgs {
            script: script_path,
            mock: Some(mock_path),
        };
        assert!(mock_check(args).is_ok());
    }

    #[test]
    fn mock_check_custom_mock_path() {
        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("wf.lua");
        let mock_path = dir.path().join("custom_name.json");
        std::fs::write(&script_path, build_script(&["a"])).unwrap();
        std::fs::write(&mock_path, build_mock(&["a"], false)).unwrap();

        let args = MockCheckArgs {
            script: script_path,
            mock: Some(mock_path),
        };
        assert!(mock_check(args).is_ok());
    }

    #[test]
    fn mock_check_missing_mock_file_fails() {
        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("wf.lua");
        std::fs::write(&script_path, build_script(&["a"])).unwrap();

        let args = MockCheckArgs {
            script: script_path,
            mock: Some(dir.path().join("does_not_exist.json")),
        };
        assert!(mock_check(args).is_err());
    }

    #[test]
    fn mock_check_invalid_json_fails() {
        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("wf.lua");
        let mock_path = dir.path().join("wf.lua.mock.json");
        std::fs::write(&script_path, build_script(&["a"])).unwrap();
        std::fs::write(&mock_path, "{not valid json").unwrap();

        let args = MockCheckArgs {
            script: script_path,
            mock: Some(mock_path),
        };
        assert!(mock_check(args).is_err());
    }
}
