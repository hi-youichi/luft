//! `mock add` subcommand: generate mock data for an existing Lua script.

use crate::backend;
use anyhow::Result;
use maestro::mock_gen::{generate_mock_for_script, MockGenConfig};
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
        eprintln!("\u{2139}  no --backend specified, auto-detected: {}", backend_id);
    }

    let config = crate::config::load_config();
    let model = crate::config::resolve_planner_model(
        args.model.as_deref(),
        None,
        config.as_ref().and_then(|c| c.planner.model.as_deref()),
    );

    let script = std::fs::read_to_string(&args.script)
        .map_err(|e| anyhow::anyhow!("cannot read '{}': {}", args.script.display(), e))?;

    let calls = maestro::mock_gen::extract_agent_calls(&script);

    if calls.is_empty() {
        anyhow::bail!("no agent() calls found in {}", args.script.display());
    }

    let unnamed: Vec<&maestro::mock_gen::AgentCallInfo> =
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
    eprintln!("    {} responses, {} agent calls", response_count, names.len());

    Ok(())
}
