//! `maestro` binary entry point.
//!
//! CLI commands:
//! - `maestro run "<NL>"` — NL → Lua via planner, then execute
//! - `maestro run /<wf>` — Run workflow.lua file
//! - `maestro run --resume` — Resume from checkpoint
//! - `maestro run --confirm` — Show script for confirmation (default: auto-approve)
//!
//! `main` only parses args and routes each subcommand to its handler in
//! [`commands`]; the backend factory lives in [`backend`].

mod backend;
mod commands;
mod logging;

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
    /// Program-log level (trace|debug|info|warn|error). Overrides RUST_LOG.
    #[arg(long, global = true)]
    log_level: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a Lua workflow script from a natural language prompt (no execution).
    Generate(GenerateArgs),
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
        #[arg(help = "Run directory name to inspect")]
        run_dir: String,
    },
    /// Show event log for a past run.
    Logs {
        #[arg(short, long, help = "Limit to N events")]
        limit: Option<usize>,
        #[arg(help = "Run directory name to inspect")]
        run_dir: String,
    },
    /// MCP server subcommand for structured output injection (internal).
    #[command(hide = true)]
    McpStructuredOutput(commands::mcp_server::McpStructuredOutputArgs),
}

#[derive(clap::Args)]
struct GenerateArgs {
    #[arg(help = "Natural language prompt describing the workflow to generate")]
    nl: String,

    #[arg(short, long, help = "Write generated Lua script to this file (default: stdout)")]
    output: Option<PathBuf>,

    #[arg(short, long, help = "Backend to use (default: auto-detect opencode, fallback mock)")]
    backend: Option<String>,
}

#[derive(clap::Args)]
struct RunArgs {
    #[arg(help = "Natural language prompt (auto-generates Lua script)")]
    nl: Option<String>,

    #[arg(short, long, help = "Path to workflow.lua file")]
    workflow: Option<PathBuf>,

    #[arg(short, long, help = "Resume from last checkpoint")]
    resume: bool,

    #[arg(short, long, help = "Show script for confirmation before execution (default: auto-approve)")]
    confirm: bool,

    #[arg(short, long, help = "Backend to use (default: auto-detect opencode, fallback mock)")]
    backend: Option<String>,

    #[arg(long, help = "Disable raw ACP session/update passthrough (acp_raw events)")]
    no_acp_raw: bool,

    #[arg(long, help = "Write the event log to this file (in addition to normal output)")]
    log: Option<PathBuf>,

    #[arg(long, value_enum, default_value_t = commands::event_log::LogFormat::Pretty, help = "Event log format")]
    log_format: commands::event_log::LogFormat,

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

    logging::init(cli.log_level.as_deref(), "warn")?;

    match cli.command {
        Commands::Generate(args) => commands::generate::generate_script(args).await?,
        Commands::Run(args) => commands::run::run_workflow(args).await?,
        Commands::Workflows => commands::workflows::list_workflows()?,
        Commands::Save { name, output } => commands::save::save_workflow(&name, &output)?,
        Commands::List { limit } => commands::list::list_runs_cmd(limit)?,
        Commands::Status { run_dir } => commands::status::status_run_cmd(run_dir)?,
        Commands::Logs { run_dir, limit } => commands::logs::logs_run_cmd(run_dir, limit)?,
        Commands::McpStructuredOutput(args) => commands::mcp_server::run(args)?,
    }

    Ok(())
}
