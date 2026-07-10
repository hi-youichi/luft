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
mod config;
mod logging;
mod signal;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tokio::sync::broadcast;

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
    /// Write program-log (tracing) to this file. Default: `~/.maestro/logs/maestro.log`.
    #[arg(long, global = true, value_name = "FILE")]
    log_file: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
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
    /// Clear terminal-state runs (completed/cancelled/failed). Running runs are preserved.
    Clear {
        #[arg(long, help = "Only clear runs older than N days")]
        days: Option<u64>,
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
    /// Manage backends (list / info / check).
    #[command(subcommand)]
    Backend(commands::backend::BackendSubcommand),
    /// Show planned phases for a past run (uses the script's `meta` table when available).
    Phases {
        #[arg(help = "Run directory name to inspect")]
        run_dir: String,
        #[arg(long, help = "Render phases view as JSON instead of a plain-text table")]
        json: bool,
    },
    /// MCP server subcommand for structured output injection (internal).
    #[command(hide = true)]
    McpStructuredOutput(commands::mcp_server::McpStructuredOutputArgs),
    /// Start the Maestro MCP server (stdio JSON-RPC).
    #[command(subcommand)]
    Mcp(commands::mcp_server::McpSubcommand),
    /// Lua script utilities.
    #[command(subcommand)]
    Lua(commands::lua_validate::LuaSubcommand),
    /// Generate mock data for an existing Lua script.
    #[command(subcommand)]
    Mock(commands::mock::MockSubcommand),
}

#[derive(Debug, clap::Args)]
struct GenerateArgs {
    #[arg(help = "Natural language prompt describing the workflow to generate")]
    nl: String,

    #[arg(
        short,
        long,
        help = "Write generated Lua script to this file (default: stdout)"
    )]
    output: Option<PathBuf>,

    #[arg(short, long, help = "Backend to use (default: auto-detect or prompt)")]
    backend: Option<String>,

    #[arg(long, help = "Model for NL→Lua planning (overrides config)")]
    model: Option<String>,

    #[arg(long, help = "Also generate a .mock.json companion file")]
    with_mock: bool,
}

#[derive(Debug, clap::Args)]
struct RunArgs {
    #[arg(help = "Natural language prompt (auto-generates Lua script)")]
    nl: Option<String>,

    #[arg(short, long, help = "Path to workflow.lua file")]
    workflow: Option<PathBuf>,

    #[arg(short, long, help = "Resume from last checkpoint")]
    resume: bool,

    #[arg(
        short,
        long,
        help = "Show script for confirmation before execution (default: auto-approve)"
    )]
    confirm: bool,

    #[arg(short, long, help = "Backend to use (default: auto-detect or prompt)")]
    backend: Option<String>,

    #[arg(
        long,
        help = "Disable raw ACP session/update passthrough (acp_raw events)"
    )]
    no_acp_raw: bool,

    #[arg(
        long,
        help = "Write the event log to this file (in addition to normal output)"
    )]
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

    #[arg(long, help = "Auto-fix script on execution failure (default: off)")]
    auto_fix: bool,

    #[arg(
        long,
        default_value_t = 3,
        help = "Max fix attempts when auto-fix is enabled"
    )]
    max_fix_attempts: u32,

    #[arg(long, help = "Disable writing agent artifact reports to disk")]
    no_artifacts: bool,

    #[arg(long, help = "Show MCP tool call summary after execution")]
    verbose: bool,

    #[arg(long, help = "Model to use for LLM calls (overrides config)")]
    model: Option<String>,

    #[arg(
        long,
        help = "Model for NL→Lua planning only (overrides --model for planner)"
    )]
    planner_model: Option<String>,

    #[arg(
        long,
        help = "Max number of agents running in parallel (default: auto 4-16)"
    )]
    max_concurrency: Option<usize>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cancel = tokio_util::sync::CancellationToken::new();
    let (sig_tx, _) = broadcast::channel(16);
    signal::install(sig_tx.clone(), cancel.clone());
    dispatch(cli, cancel, sig_tx).await
}

/// Dispatch a parsed CLI to the appropriate handler.
/// Exposed as a separate function so it can be tested without touching
/// process-global `std::env::args()`.
async fn dispatch(
    cli: Cli,
    cancel: tokio_util::sync::CancellationToken,
    sig_tx: broadcast::Sender<signal::SignalInfo>,
) -> Result<()> {
    logging::init(cli.log_level.as_deref(), "warn", cli.log_file.as_deref())?;

    match cli.command {
        Commands::Generate(args) => commands::generate::generate_script(args).await?,
        Commands::Run(args) => {
            commands::run::run_workflow(args, cancel.clone(), sig_tx.clone()).await?
        }
        Commands::Workflows => commands::workflows::list_workflows()?,
        Commands::Save { name, output } => commands::save::save_workflow(&name, &output)?,
        Commands::List { limit } => commands::list::list_runs_cmd(limit)?,
        Commands::Clear { days } => commands::clear::clear_runs_cmd(days)?,
        Commands::Status { run_dir } => commands::status::status_run_cmd(run_dir)?,
        Commands::Logs { run_dir, limit } => commands::logs::logs_run_cmd(run_dir, limit)?,
        Commands::Backend(cmd) => match cmd {
            commands::backend::BackendSubcommand::List => commands::backend::list_backends(),
            commands::backend::BackendSubcommand::Info { id } => {
                commands::backend::info_backend(id);
            }
            commands::backend::BackendSubcommand::Check { id } => {
                commands::backend::check_backend(id);
            }
            commands::backend::BackendSubcommand::Config { .. } => {
                anyhow::bail!("backend config subcommand is not yet implemented");
            }
            commands::backend::BackendSubcommand::Set { .. } => {
                anyhow::bail!("backend set subcommand is not yet implemented");
            }
        },
        Commands::Phases { run_dir, json } => {
            commands::phases::phases_cmd(run_dir, commands::phases::PhasesArgs { json })?;
        }
        Commands::McpStructuredOutput(args) => commands::mcp_server::run(args)?,
        Commands::Mcp(cmd) => match cmd {
            commands::mcp_server::McpSubcommand::Serve(args) => {
                commands::mcp_server::serve(args).await?
            }
        },
        Commands::Lua(cmd) => match cmd {
            commands::lua_validate::LuaSubcommand::Validate(args) => {
                commands::lua_validate::validate_lua(args)?
            }
            commands::lua_validate::LuaSubcommand::MockCheck(args) => {
                commands::lua_validate::mock_check(args)?
            }
        },
        Commands::Mock(cmd) => match cmd {
            commands::mock::MockSubcommand::Add(args) => commands::mock::mock_add(args).await?,
        },
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dispatch_generate_unknown_backend() {
        let cli = Cli {
            command: Commands::Generate(GenerateArgs {
                nl: "do something".into(),
                output: None,
                backend: Some("does-not-exist".into()),
                model: None,
                with_mock: false,
            }),
            log_level: Some("debug".into()),
            log_file: None,
        };
        let err = dispatch(
            cli,
            tokio_util::sync::CancellationToken::new(),
            tokio::sync::broadcast::channel::<signal::SignalInfo>(16).0,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("unknown backend"),
            "expected 'unknown backend' error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn dispatch_generate_mock_backend_planner_exhausted() {
        let cli = Cli {
            command: Commands::Generate(GenerateArgs {
                nl: "do something".into(),
                output: None,
                backend: Some("mock".into()),
                model: None,
                with_mock: false,
            }),
            log_level: None,
            log_file: None,
        };
        let err = dispatch(
            cli,
            tokio_util::sync::CancellationToken::new(),
            tokio::sync::broadcast::channel::<signal::SignalInfo>(16).0,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("planner") || err.to_string().contains("real LLM backend"),
            "expected planner or backend error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn dispatch_run_unknown_backend() {
        let cli = Cli {
            command: Commands::Run(RunArgs {
                nl: Some("do something".into()),
                workflow: None,
                resume: false,
                confirm: false,
                backend: Some("does-not-exist".into()),
                no_acp_raw: false,
                log: None,
                log_format: commands::event_log::LogFormat::Pretty,
                output: None,
                args_json: None,
                extra_args: None,
                auto_fix: false,
                max_fix_attempts: 3,
                no_artifacts: false,
                verbose: false,
                model: None,
                planner_model: None,
                max_concurrency: None,
            }),
            log_level: None,
            log_file: None,
        };
        let err = dispatch(
            cli,
            tokio_util::sync::CancellationToken::new(),
            tokio::sync::broadcast::channel::<signal::SignalInfo>(16).0,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("unknown backend"),
            "expected 'unknown backend' error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn dispatch_run_without_nl_or_workflow() {
        let cli = Cli {
            command: Commands::Run(RunArgs {
                nl: None,
                workflow: None,
                resume: false,
                confirm: false,
                backend: Some("mock".into()),
                no_acp_raw: false,
                log: None,
                log_format: commands::event_log::LogFormat::Pretty,
                output: None,
                args_json: None,
                extra_args: None,
                auto_fix: false,
                max_fix_attempts: 3,
                no_artifacts: false,
                verbose: false,
                model: None,
                planner_model: None,
                max_concurrency: None,
            }),
            log_level: None,
            log_file: None,
        };
        let err = dispatch(
            cli,
            tokio_util::sync::CancellationToken::new(),
            tokio::sync::broadcast::channel::<signal::SignalInfo>(16).0,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("natural language prompt")
                || err.to_string().contains("--workflow"),
            "expected NL/workflow error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn dispatch_workflows() {
        let cli = Cli {
            command: Commands::Workflows,
            log_level: None,
            log_file: None,
        };
        dispatch(
            cli,
            tokio_util::sync::CancellationToken::new(),
            tokio::sync::broadcast::channel::<signal::SignalInfo>(16).0,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dispatch_save() {
        let cli = Cli {
            command: Commands::Save {
                name: "test".into(),
                output: PathBuf::from("out.lua"),
            },
            log_level: None,
            log_file: None,
        };
        let err = dispatch(
            cli,
            tokio_util::sync::CancellationToken::new(),
            tokio::sync::broadcast::channel::<signal::SignalInfo>(16).0,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("not implemented"),
            "expected 'not implemented' error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn dispatch_list() {
        let cli = Cli {
            command: Commands::List { limit: None },
            log_level: None,
            log_file: None,
        };
        dispatch(
            cli,
            tokio_util::sync::CancellationToken::new(),
            tokio::sync::broadcast::channel::<signal::SignalInfo>(16).0,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn dispatch_status_not_found() {
        let cli = Cli {
            command: Commands::Status {
                run_dir: "__nonexistent_run__".into(),
            },
            log_level: None,
            log_file: None,
        };
        let err = dispatch(
            cli,
            tokio_util::sync::CancellationToken::new(),
            tokio::sync::broadcast::channel::<signal::SignalInfo>(16).0,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("not found") || err.to_string().contains("No such file"),
            "expected 'not found' error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn dispatch_logs_not_found() {
        let cli = Cli {
            command: Commands::Logs {
                run_dir: "__nonexistent_run__".into(),
                limit: None,
            },
            log_level: None,
            log_file: None,
        };
        let err = dispatch(
            cli,
            tokio_util::sync::CancellationToken::new(),
            tokio::sync::broadcast::channel::<signal::SignalInfo>(16).0,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("not found"),
            "expected 'not found' error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn dispatch_mcp_no_schema_file() {
        let cli = Cli {
            command: Commands::McpStructuredOutput(commands::mcp_server::McpStructuredOutputArgs {
                schema_file: PathBuf::from("/__nonexistent__/schema.json"),
            }),
            log_level: None,
            log_file: None,
        };
        let err = dispatch(
            cli,
            tokio_util::sync::CancellationToken::new(),
            tokio::sync::broadcast::channel::<signal::SignalInfo>(16).0,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("No such file")
                || err.to_string().contains("os error 2")
                || err.to_string().contains("os error 3")
                || err.to_string().contains("cannot find the path"),
            "expected filesystem error, got: {}",
            err
        );
    }
}
