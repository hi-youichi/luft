//! `maestro` binary entry point.
//!
//! CLI commands:
//! - `maestro run "<NL>"` — NL → Lua via planner, then execute
//! - `maestro run /<wf>` — Run workflow.lua file
//! - `maestro run --resume` — Resume from checkpoint
//! - `maestro run --headless` — JSONL output mode
//! - `maestro run --confirm` — Show script for confirmation (default: auto-approve)
//!
//! `main` only parses args and routes each subcommand to its handler in
//! [`commands`]; the backend factory lives in [`backend`].

mod backend;
mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    /// Run the WebSocket server (clients drive runs over the WS protocol).
    Serve {
        #[arg(long, default_value = "0.0.0.0:8080", help = "Address to bind")]
        addr: String,
        #[arg(short, long, help = "Backend to use (default: auto-detect)")]
        backend: Option<String>,
        #[arg(long, default_value_t = 4, help = "Max concurrent runs")]
        max_concurrent: usize,
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
        Commands::Run(args) => commands::run::run_workflow(args).await?,
        Commands::Workflows => commands::workflows::list_workflows()?,
        Commands::Save { name, output } => commands::save::save_workflow(&name, &output)?,
        Commands::List { limit } => commands::list::list_runs_cmd(limit)?,
        Commands::Status { run_id } => commands::status::status_run_cmd(run_id)?,
        Commands::Logs { run_id, limit } => commands::logs::logs_run_cmd(run_id, limit)?,
        Commands::Serve { addr, backend, max_concurrent } => {
            commands::serve::serve_cmd(addr, backend, max_concurrent).await?
        }
    }

    Ok(())
}
