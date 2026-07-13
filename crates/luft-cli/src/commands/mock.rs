//! `mock add` subcommand: generate mock data for an existing Lua script.

use crate::backend;
use anyhow::Result;
use luft::mock_gen::{generate_mock_for_script, MockGenConfig};
use std::path::PathBuf;

#[derive(Debug, clap::Subcommand)]
pub enum MockSubcommand {
    /// Generate mock data for an existing Lua script.
    Add(MockAddArgs),
}

#[derive(Debug, clap::Args)]
pub struct MockAddArgs {
    /// Path to the .lua workflow script.
    pub script: PathBuf,

    /// Output path (default: <script>.mock.json).
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Backend to use (default: auto-detect or prompt).
    #[arg(short, long)]
    pub backend: Option<String>,

    /// Model override.
    #[arg(long)]
    pub model: Option<String>,

    /// Max LLM retry attempts (default: 3).
    #[arg(long)]
    pub max_retries: Option<usize>,
}

pub async fn mock_add(args: MockAddArgs) -> Result<()> {
    let backend_id = crate::config::resolve_default_backend(args.backend.as_deref());
    if backend_id == "mock" || backend_id == "mockfile" {
        anyhow::bail!(
            "mock add requires a real LLM backend. \
             Specify --backend <id> (e.g. opencode)."
        );
    }
    if args.backend.is_none() {
        eprintln!(
            "\u{2139}  no --backend specified, auto-detected: {}",
            backend_id
        );
    }

    let config = crate::config::load_config();
    let model = crate::config::resolve_planner_model(
        args.model.as_deref(),
        None,
        config.as_ref().and_then(|c| c.planner.model.as_deref()),
    );

    let script = std::fs::read_to_string(&args.script)
        .map_err(|e| anyhow::anyhow!("cannot read '{}': {}", args.script.display(), e))?;

    let calls = luft::mock_gen::extract_agent_calls(&script);

    if calls.is_empty() {
        anyhow::bail!("no agent() calls found in {}", args.script.display());
    }

    let unnamed: Vec<&luft::mock_gen::AgentCallInfo> =
        calls.iter().filter(|c| c.name.is_none()).collect();
    if !unnamed.is_empty() {
        eprintln!(
            "\u{2717}  Found {} agent() calls without name=:",
            unnamed.len()
        );
        for call in &unnamed {
            eprintln!("    line {}: agent(...)", call.line);
        }
        eprintln!("\nEvery agent() call must include a unique name= field:");
        eprintln!("  agent({{ name = \"analyze\", prompt = \"...\" }})");
        anyhow::bail!("add name= fields to your script, then re-run");
    }

    let names: Vec<&str> = calls.iter().filter_map(|c| c.name.as_deref()).collect();
    eprintln!(
        "\u{2699}  Generating mock data for {} agent call(s): {}",
        names.len(),
        names.join(", ")
    );

    let backend = backend::create_backend(&backend_id, false, model.clone())?;
    let cfg = MockGenConfig {
        model,
        max_retries: args.max_retries.unwrap_or(3),
    };

    let mock_data = generate_mock_for_script(&script, backend, &cfg).await?;

    let mock_path = args
        .output
        .clone()
        .unwrap_or_else(|| args.script.with_extension("mock.json"));

    let json = serde_json::to_string_pretty(&mock_data)?;
    std::fs::write(&mock_path, json + "\n")?;

    eprintln!("\u{2705}  Mock data written to {}", mock_path.display());

    let response_count = mock_data
        .get("responses")
        .and_then(|r| r.as_object())
        .map(|m| m.len())
        .unwrap_or(0);
    eprintln!(
        "    {} responses, {} agent calls",
        response_count,
        names.len()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_args(script: std::path::PathBuf) -> MockAddArgs {
        MockAddArgs {
            script,
            output: None,
            backend: None,
            model: None,
            max_retries: None,
        }
    }

    #[test]
    fn mock_add_args_debug() {
        let args = MockAddArgs {
            script: std::path::PathBuf::from("/tmp/x.lua"),
            output: Some(std::path::PathBuf::from("/tmp/out.json")),
            backend: Some("opencode".into()),
            model: Some("claude-3".into()),
            max_retries: Some(5),
        };
        let s = format!("{args:?}");
        assert!(s.contains("script"));
        assert!(s.contains("/tmp/x.lua"));
        assert!(s.contains("opencode"));
        assert!(s.contains("max_retries"));
    }

    #[test]
    fn mock_add_args_default_values_when_all_none() {
        let args = MockAddArgs {
            script: std::path::PathBuf::from("a.lua"),
            output: None,
            backend: None,
            model: None,
            max_retries: None,
        };
        assert!(args.output.is_none());
        assert!(args.backend.is_none());
        assert!(args.model.is_none());
        assert!(args.max_retries.is_none());
    }

    // The clap-derived Subcommand/Args structs are tested at compile time;
    // this runtime test ensures Debug is callable and exposes the variant.
    #[test]
    fn mock_subcommand_add_variant_debug() {
        let sub = MockSubcommand::Add(make_args(std::path::PathBuf::from("a.lua")));
        let s = format!("{sub:?}");
        assert!(s.contains("Add"));
    }

    // ── mock_add runtime tests ───────────────────────────────────────────

    #[tokio::test]
    async fn mock_add_bails_when_backend_is_mock() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("w.lua");
        std::fs::write(&script, "agent({ name = \"x\", prompt = \"y\" })").unwrap();

        let args = MockAddArgs {
            script,
            output: None,
            backend: Some("mock".to_string()),
            model: None,
            max_retries: None,
        };
        let err = mock_add(args).await.unwrap_err();
        assert!(
            err.to_string().contains("real LLM backend") || err.to_string().contains("mock"),
            "expected mock-related error, got: {err}"
        );
    }

    #[tokio::test]
    async fn mock_add_bails_when_backend_is_mockfile() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("w.lua");
        std::fs::write(&script, "agent({ name = \"x\", prompt = \"y\" })").unwrap();

        let args = MockAddArgs {
            script,
            output: None,
            backend: Some("mockfile".to_string()),
            model: None,
            max_retries: None,
        };
        let err = mock_add(args).await.unwrap_err();
        assert!(
            err.to_string().contains("real LLM backend") || err.to_string().contains("mock"),
            "expected mock-related error, got: {err}"
        );
    }

    #[tokio::test]
    async fn mock_add_errors_when_script_does_not_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("does-not-exist.lua");

        let args = MockAddArgs {
            script,
            output: None,
            backend: Some("opencode".to_string()),
            model: None,
            max_retries: None,
        };
        let err = mock_add(args).await.unwrap_err();
        assert!(
            err.to_string().contains("cannot read")
                || err.to_string().contains("os error 2")
                || err.to_string().contains("No such file"),
            "expected file-read error, got: {err}"
        );
    }

    #[tokio::test]
    async fn mock_add_bails_when_no_agent_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("empty.lua");
        std::fs::write(&script, "-- empty script\nx = 1\n").unwrap();

        let args = MockAddArgs {
            script,
            output: None,
            backend: Some("opencode".to_string()),
            model: None,
            max_retries: None,
        };
        let err = mock_add(args).await.unwrap_err();
        assert!(
            err.to_string().contains("no agent() calls"),
            "expected 'no agent calls' error, got: {err}"
        );
    }

    #[tokio::test]
    async fn mock_add_bails_when_calls_have_no_name() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("noname.lua");
        std::fs::write(&script, "agent({ prompt = \"no name here\" })").unwrap();

        let args = MockAddArgs {
            script,
            output: None,
            backend: Some("opencode".to_string()),
            model: None,
            max_retries: None,
        };
        let err = mock_add(args).await.unwrap_err();
        assert!(
            err.to_string().contains("name="),
            "expected error mentioning name=, got: {err}"
        );
    }

    #[tokio::test]
    async fn mock_add_reports_line_numbers_for_unnamed_calls() {
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("mixed.lua");
        // First line is comment, then unnamed agent at line 2, named at line 3,
        // unnamed again at line 4.
        std::fs::write(
            &script,
            "-- header\nagent({ prompt = \"a\" })\nagent({ name = \"ok\", prompt = \"b\" })\nagent({ prompt = \"c\" })\n",
        )
        .unwrap();

        let args = MockAddArgs {
            script,
            output: None,
            backend: Some("opencode".to_string()),
            model: None,
            max_retries: None,
        };
        let err = mock_add(args).await.unwrap_err();
        // The error message says add name= fields. Use stderr to verify the
        // diagnostic eprintln output mentioning the count and lines. Capture
        // the diagnostics by reading the original eprintln call directly.
        // Instead of parsing eprintln, verify that the user gets a clear hint.
        let msg = err.to_string();
        assert!(
            msg.contains("name=") || msg.contains("re-run"),
            "expected hint about adding name= fields, got: {msg}"
        );
    }

    #[test]
    fn unnamed_agent_call_diagnostic_format() {
        // Re-test the same flow synchronously by exercising
        // extract_agent_calls with multiple unnamed calls to confirm the parser
        // correctly identifies the offending lines.
        let script =
            "agent({ prompt = \"a\" })\nagent({ name = \"ok\" })\nagent({ prompt = \"c\" })";
        let calls = luft::mock_gen::extract_agent_calls(script);
        let unnamed: Vec<_> = calls.iter().filter(|c| c.name.is_none()).collect();
        assert_eq!(unnamed.len(), 2);
        assert_eq!(unnamed[0].line, 1);
        assert_eq!(unnamed[1].line, 3);
    }

    // Sanity: the `extract_agent_calls` helper from luft::mock_gen is the
    // source of truth for parsing. Make sure it surfaces the calls we feed
    // through mock_add before the bail-out check fires.
    #[test]
    fn extract_agent_calls_finds_named_and_unnamed() {
        let script = "agent({ name = \"a\", prompt = \"p\" })\nagent({ prompt = \"q\" })";
        let calls = luft::mock_gen::extract_agent_calls(script);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].name.as_deref(), Some("a"));
        assert!(calls[1].name.is_none());
    }

    #[test]
    fn extract_agent_calls_returns_empty_for_no_calls() {
        let calls = luft::mock_gen::extract_agent_calls("x = 1\nprint(x)\n");
        assert!(calls.is_empty());
    }
}
