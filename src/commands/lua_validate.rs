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
        eprintln!("FAIL {} - {} error(s):", file, result.errors.len());
        for err in &result.errors {
            eprintln!("  - {}", err);
        }
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

    println!("\nmain():    {}", yn(result.has_main));
    println!("report():  {}", yn(result.has_report_call));
    println!("phase_begin/end paired: {}", yn(result.span_pairing_ok));

    if !result.warnings.is_empty() {
        println!("\nWarnings:");
        for w in &result.warnings {
            println!("  - {}", w);
        }
    }

    if result.is_valid() {
        println!("\nok {} - valid", file);
    } else {
        eprintln!("\nFAIL {} - {} error(s):", file, result.errors.len());
        for err in &result.errors {
            eprintln!("  - {}", err);
        }
    }
}

fn yn(ok: bool) -> &'static str {
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
        anyhow::anyhow!("invalid mock JSON: {}. Expected format: {{\"responses\": {{...}}}}", e)
    })?;

    let mock_responses = mock_file
        .get("responses")
        .and_then(|r| r.as_object())
        .cloned()
        .unwrap_or_default();

    let has_default = mock_file.get("default").is_some();

    let lua_names = extract_agent_names(&script);
    let lua_names_set: std::collections::HashSet<&str> =
        lua_names.iter().map(|s| s.as_str()).collect();
    let mock_names_set: std::collections::HashSet<&str> =
        mock_responses.keys().map(|s| s.as_str()).collect();

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

    println!("── Mock Coverage ──");
    println!("  Script:     {}", args.script.display());
    println!("  Mock file:  {}", mock_path.display());
    println!();
    println!("  Agent names in script: {}", lua_names.len());
    println!("  Mock responses:        {}", mock_responses.len());
    println!("  Matched:               {}", matched.len());
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
        for n in &missing {
            println!("    - {}", n);
        }
    }
    if !extra.is_empty() {
        println!();
        println!("  Extra (unused) mock entries:");
        for n in &extra {
            println!("    - {}", n);
        }
    }

    let coverage = if lua_names.is_empty() {
        100.0
    } else {
        matched.len() as f64 / lua_names.len() as f64 * 100.0
    };
    println!();
    println!("  Coverage: {:.0}%", coverage);

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

/// Extract agent names — delegates to the library implementation.
fn extract_agent_names(script: &str) -> Vec<String> {
    maestro::mock_gen::extract_agent_names(script)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_tmp(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f
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
}
