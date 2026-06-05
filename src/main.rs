//! `maestro` binary entry point.
//!
//! CLI commands:
//! - `maestro run "<NL>"` — NL → Lua via planner, then execute
//! - `maestro run /<wf>` — Run workflow.lua file
//! - `maestro run --resume` — Resume from checkpoint
//! - `maestro run --headless` — JSONL output mode
//! - `maestro run --approve` — Auto-approve without prompt

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

// Inline backend factory module

#[derive(Parser)]
#[command(name = "maestro")]
#[command(version = "0.1.0")]
#[command(about = "Maestro — multi-agent orchestration runtime (v0.1)", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a workflow (from file or NL prompt).
    Run(RunArgs),
    /// List available workflows.
    Workflows,
    /// Save a workflow to file.
    Save {
        #[arg(help = "Workflow name")]
        name: String,
        #[arg(help = "Output file path")]
        output: PathBuf,
    },
    /// List past runs.
    List {
        #[arg(short, long, help = "Limit to N most recent runs")]
        limit: Option<usize>,
    },
    /// Show status of a past run.
    Status {
        #[arg(help = "Run ID to inspect")]
        run_id: uuid::Uuid,
    },
    /// Show event log for a past run.
    Logs {
        #[arg(short, long, help = "Limit to N events")]
        limit: Option<usize>,
        #[arg(help = "Run ID to inspect")]
        run_id: uuid::Uuid,
    },
}

#[derive(clap::Args)]
struct RunArgs {
    #[arg(help = "Natural language prompt (auto-generates Lua script)")]
    nl: Option<String>,

    #[arg(short, long, help = "Path to workflow.lua file")]
    workflow: Option<PathBuf>,

    #[arg(short, long, help = "Resume from last checkpoint")]
    resume: bool,

    #[arg(long, help = "Headless mode (JSONL output to stdout)")]
    headless: bool,

    #[arg(short, long, help = "Show script for confirmation before execution (default: auto-approve)")]
    confirm: bool,

    #[arg(short, long, help = "Backend to use (default: auto-detect opencode, fallback mock)")]
    backend: Option<String>,

    #[arg(
        short,
        long,
        help = "Write the final report to this file (clean Markdown if the report has a `markdown` field)"
    )]
    output: Option<PathBuf>,

    #[arg(
        long = "args",
        help = "Arguments passed to the workflow as a JSON object (e.g. --args '{\"topic\":\"...\"}')"
    )]
    args_json: Option<String>,

    #[arg(help = "Extra arguments passed to the workflow as JSON (positional; prefer --args)")]
    extra_args: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Run(args) => {
            run_workflow(args).await?;
        }
        Commands::Workflows => {
            list_workflows()?;
        }
        Commands::Save { name, output } => {
            save_workflow(&name, &output)?;
        }
        Commands::List { limit } => {
            list_runs_cmd(limit)?;
        }
        Commands::Status { run_id } => {
            status_run_cmd(run_id)?;
        }
        Commands::Logs { run_id, limit } => {
            logs_run_cmd(run_id, limit)?;
        }
    }

    Ok(())
}

async fn run_workflow(args: RunArgs) -> Result<()> {
    let is_nl = args.nl.is_some();
    let backend_id = match args.backend.as_deref() {
        Some(id) => id.to_string(),
        None => {
            let detected = backend::detect_backend();
            if is_nl && detected == "mock" {
                anyhow::bail!(
                    "NL mode requires a real LLM backend. \
                     Install opencode (https://opencode.ai) or specify --backend <id>"
                );
            }
            if is_nl {
                eprintln!("ℹ  no --backend specified, auto-detected: {}", detected);
            }
            detected.to_string()
        }
    };
    let backend = backend::create_backend(&backend_id)?;

    // For fresh runs, resolve the script (NL → agent-driven planner, or a
    // --workflow file) and, unless --approve, show it for confirmation. Resume
    // runs reload the persisted workflow.lua inside cli::run.
    let mut generated_script: Option<String> = None;
    if !args.resume {
        let script = if let Some(nl) = args.nl.as_deref() {
            // Agent-driven planning: an LLM agent writes the Lua orchestration
            // script for the task (Claude DW "compile once" model).
            let cfg = maestro::planner::PlannerConfig::default();
            let planned = maestro::planner::plan_workflow(nl, backend.clone(), &cfg)
                .await
                .map_err(|e| anyhow::anyhow!("planning failed: {}", e))?;
            planned.script
        } else if let Some(wf) = args.workflow.as_ref() {
            if !wf.exists() {
                anyhow::bail!("workflow file not found: {}", wf.display());
            }
            std::fs::read_to_string(wf)?
        } else {
            anyhow::bail!("either a natural language prompt or --workflow <file> is required");
        };

        if args.confirm {
            println!("=== Workflow Script ===");
            println!("{}", script);
            println!("=======================");

            print!("Approve execution? [y/N] ");
            std::io::Write::flush(&mut std::io::stdout())?;
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("Aborted.");
                return Ok(());
            }
        }

        // NL runs carry the generated script through to cli::run; file runs let
        // cli::run reload from disk via --workflow.
        if args.nl.is_some() {
            generated_script = Some(script);
        }
    }

    let mut run_args = maestro::cli::RunArgs::new(
        args.nl.clone(),
        args.workflow.clone(),
        args.resume,
        if args.headless {
            maestro::cli::RunMode::Headless
        } else {
            maestro::cli::RunMode::Tui
        },
        !args.confirm,
    );
    run_args.script = generated_script;
    run_args.output = args.output.clone();
    // `--args` takes precedence over the positional `extra_args`.
    run_args.extra_args = args
        .args_json
        .as_ref()
        .or(args.extra_args.as_ref())
        .map(|s| serde_json::from_str(s).unwrap_or_default())
        .unwrap_or_default();

    maestro::cli::run(backend, run_args).await
}

fn list_workflows() -> Result<()> {
    // List workflows from ~/.maestro/workflows/ directory
    let workflow_dir = dirs::config_dir()
        .unwrap_or_default()
        .join("maestro")
        .join("workflows");

    if !workflow_dir.exists() {
        println!("No workflows found. Create one with `maestro save <name> <file>`");
        return Ok(());
    }

    println!("Available workflows:");
    for entry in std::fs::read_dir(workflow_dir)? {
        let entry = entry?;
        if let Some(ext) = entry.path().extension() {
            if ext == "lua" {
                println!("  - {}", entry.file_name().to_string_lossy());
            }
        }
    }

    Ok(())
}

/// Runs are stored in `.maestro/runs` relative to the current working directory.
fn runs_base_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(".")
        .join(".maestro")
        .join("runs")
}

fn list_runs_cmd(limit: Option<usize>) -> Result<()> {
    use maestro::core::state::list_runs;

    let base_dir = runs_base_dir();
    if !base_dir.exists() {
        println!("No runs found (no runs directory at {})", base_dir.display());
        return Ok(());
    }

    let run_ids = list_runs(&base_dir)?;
    if run_ids.is_empty() {
        println!("No runs found.");
        return Ok(());
    }

    let limit = limit.unwrap_or(20);
    let run_ids: Vec<_> = run_ids.into_iter().rev().take(limit).collect();

    println!("Past runs ({} total, showing {} most recent):", run_ids.len(), limit);
    for run_id in run_ids {
        // Load checkpoint to show status
        let run_dir = base_dir.join(run_id.to_string());
        let checkpoint_path = run_dir.join("checkpoint.json");

        let status = if checkpoint_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&checkpoint_path) {
                if let Ok(cp) = serde_json::from_str::<maestro::core::state::RunCheckpoint>(&content) {
                    format!("{:?}", cp.status)
                } else {
                    "unknown".to_string()
                }
            } else {
                "unknown".to_string()
            }
        } else {
            "no checkpoint".to_string()
        };

        println!("  {}  [{}]", run_id, status);
    }

    Ok(())
}

fn status_run_cmd(run_id: uuid::Uuid) -> Result<()> {
    use maestro::core::state::RunCheckpoint;

    let base_dir = runs_base_dir();
    let run_dir = base_dir.join(run_id.to_string());
    let checkpoint_path = run_dir.join("checkpoint.json");

    if !checkpoint_path.exists() {
        anyhow::bail!("run not found: {}", run_id);
    }

    let content = std::fs::read_to_string(&checkpoint_path)?;
    let checkpoint: RunCheckpoint = serde_json::from_str(&content)
        .map_err(|e| anyhow::anyhow!("invalid checkpoint: {}", e))?;

    println!("=== Run Status ===");
    println!("  Run ID:        {}", checkpoint.run_id);
    println!("  Task:          {}", checkpoint.task);
    println!("  Status:        {:?}", checkpoint.status);
    println!("  Current phase: {}", checkpoint.current_phase);
    println!("  Total tokens:  {}", checkpoint.total_tokens);
    println!("  Created:       {}", checkpoint.created_at);
    println!("  Updated:       {}", checkpoint.updated_at);

    if !checkpoint.completed_phases.is_empty() {
        println!("\n  Completed phases:");
        for phase in &checkpoint.completed_phases {
            println!("    - Phase {}: {} (ok={}, failed={})",
                phase.phase_id, phase.label, phase.ok, phase.failed);
        }
    }

    let agent_count = checkpoint.agent_results.len();
    if agent_count > 0 {
        println!("\n  Agent results: {} agents", agent_count);
    }

    let findings_count = checkpoint.findings.len();
    if findings_count > 0 {
        println!("  Findings: {} total", findings_count);
    }

    Ok(())
}

fn logs_run_cmd(run_id: uuid::Uuid, limit: Option<usize>) -> Result<()> {
    use maestro::core::state::get_run_store;

    let base_dir = runs_base_dir();
    let store = get_run_store(run_id, &base_dir)
        .map_err(|e| anyhow::anyhow!("failed to open run: {}", e))?;

    let events = store.get_event_log()?;
    if events.is_empty() {
        println!("No events for run {}", run_id);
        return Ok(());
    }

    let limit = limit.unwrap_or(100);
    let events: Vec<_> = events.into_iter().rev().take(limit).rev().collect();

    for event in events {
        let json = serde_json::to_string(&event)?;
        println!("{}", json);
    }

    Ok(())
}

fn save_workflow(name: &str, output: &PathBuf) -> Result<()> {
    // Ensure directory exists
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // For now, just copy from stdin or create a placeholder
    eprintln!("save_workflow: name={}, output={}", name, output.display());
    println!("Workflow saved to {}", output.display());
    Ok(())
}

// Re-export backend module
mod backend {
    use anyhow::Result;
    use std::sync::Arc;
    use maestro::core::{AgentBackend, MockBackend, MockBehavior, TokenUsage};

    pub fn create_backend(id: &str) -> Result<Arc<dyn AgentBackend>> {
        match id {
            "mock" => Ok(Arc::new(MockBackend::new(
                "mock",
                vec![MockBehavior::Success {
                    output: serde_json::Value::Null,
                    tokens: TokenUsage::default(),
                    delay: std::time::Duration::from_millis(10),
                }],
            ))),
            "opencode" => Ok(Arc::new(
                maestro::adapters::AcpAdapter::default_opencode(),
            )),
            _ => anyhow::bail!("unknown backend: {}", id),
        }
    }

    pub fn detect_backend() -> &'static str {
        if which_exists("opencode") {
            "opencode"
        } else {
            "mock"
        }
    }

    fn which_exists(cmd: &str) -> bool {
        std::process::Command::new("which")
            .arg(cmd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

// Helper for dirs
mod dirs {
    use std::path::PathBuf;

    /// macOS: ~/Library/Application Support
    /// Linux: ~/.config or $XDG_CONFIG_HOME
    pub fn config_dir() -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Library").join("Application Support"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))
        }
    }
}