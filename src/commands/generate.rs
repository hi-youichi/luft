//! `generate` subcommand: NL → Lua script generation without execution.

use crate::backend;
use crate::GenerateArgs;
use anyhow::Result;
use maestro::core::AgentBackend;
use std::sync::Arc;

pub async fn generate_script(args: GenerateArgs) -> Result<()> {
    let backend_id = crate::config::resolve_default_backend(args.backend.as_deref());
    if backend_id == "mock" {
        anyhow::bail!(
            "NL generation requires a real LLM backend. \
             Install opencode (https://opencode.ai) or specify --backend <id>"
        );
    }
    if args.backend.is_none() {
        eprintln!("ℹ  no --backend specified, auto-detected: {}", backend_id);
    }

    let config = crate::config::load_config();
    let model = crate::config::resolve_planner_model(
        args.model.as_deref(),
        None,
        config.as_ref().and_then(|c| c.planner.model.as_deref()),
    );

    let backend = backend::create_backend(&backend_id, false, model.clone())?;
    generate_script_with_backend(args, backend, model).await
}

async fn generate_script_with_backend(
    args: GenerateArgs,
    backend: Arc<dyn AgentBackend>,
    planner_model: Option<String>,
) -> Result<()> {
    let cfg = maestro::planner::PlannerConfig {
        planner_model,
        generate_mock: args.with_mock,
        ..Default::default()
    };

    if args.with_mock {
        eprintln!("\u{2699}  Generating Lua workflow script + mock data\u{2026}");
    } else {
        eprintln!("\u{2699}  Generating Lua workflow script\u{2026}");
    }

    let planned = maestro::planner::plan_workflow(&args.nl, backend, &cfg).await?;

    match args.output {
        Some(path) => {
            std::fs::write(&path, &planned.script)?;
            eprintln!("\u{2705}  Script written to {}", path.display());

            if args.with_mock {
                if let Some(ref mock) = planned.mock_data {
                    let mock_path = mock_file_path(&path);
                    let json = serde_json::to_string_pretty(mock)?;
                    std::fs::write(&mock_path, json + "\n")?;
                    eprintln!("\u{2705}  Mock data written to {}", mock_path.display());
                } else {
                    eprintln!("\u{26a0}  --with-mock requested but planner did not produce mock data");
                }
            }
        }
        None => {
            println!("{}", planned.script);
            if args.with_mock {
                if let Some(ref mock) = planned.mock_data {
                    eprintln!("\n--- mock.json ---");
                    eprintln!("{}", serde_json::to_string_pretty(mock)?);
                }
            }
        }
    }

    Ok(())
}

/// `foo.lua` -> `foo.mock.json` (replace extension)
fn mock_file_path(lua_path: &std::path::Path) -> std::path::PathBuf {
    lua_path.with_extension("mock.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GenerateArgs;
    use maestro::core::{MockBackend, MockBehavior, TokenUsage};
    use std::time::Duration;

    // ── helpers ──────────────────────────────────────────────────

    fn valid_lua_backend() -> Arc<dyn AgentBackend> {
        let script = "```lua\nlocal x = 1\nreport({ok = true})\n```";
        Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::json!(script),
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        ))
    }

    fn args(nl: &str, backend: Option<&str>, output: Option<&str>) -> GenerateArgs {
        GenerateArgs {
            nl: nl.to_string(),
            backend: backend.map(|s| s.to_string()),
            output: output.map(std::path::PathBuf::from),
            model: None,
            with_mock: false,
        }
    }

    fn restore_env_path(original: Option<String>) {
        match original {
            Some(ref p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }

    // ── generate_script (public API) ─────────────────────────────

    #[tokio::test]
    async fn generate_script_rejects_mock_detection() {
        let original_path = std::env::var("PATH").ok();
        let empty = tempfile::tempdir().unwrap();
        std::env::set_var("PATH", empty.path().to_str().unwrap());

        let err = generate_script(args("do stuff", None, None))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("real LLM backend"),
            "expected 'real LLM backend' error, got: {msg}"
        );

        restore_env_path(original_path);
    }

    #[tokio::test]
    async fn generate_script_rejects_unknown_backend() {
        let err = generate_script(args("do stuff", Some("does-not-exist"), None))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("unknown backend"),
            "expected 'unknown backend' error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn generate_script_with_mock_backend_propagates_planner_error() {
        let err = generate_script(args("do stuff", Some("mock"), None))
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("planner exhausted") || msg.contains("real LLM backend"),
            "expected planner or backend error, got: {msg}"
        );
    }

    // NOTE: the `None` backend + detect_backend == "opencode" path (lines 20–21)
    // is not covered here because the AcpAdapter's spawn_blocking task escapes
    // the tokio test runtime and hangs indefinitely when the child process
    // exits — the connect_timeout is not wired through to the ACP client.

    // ── generate_script_with_backend (private) ───────────────────

    #[tokio::test]
    async fn generate_script_with_backend_output_to_stdout() {
        let result =
            generate_script_with_backend(args("test task", None, None), valid_lua_backend(), None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn generate_script_with_backend_output_to_file() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();

        let result = generate_script_with_backend(
            args("test task", None, Some(&path.to_string_lossy())),
            valid_lua_backend(),
            None,
        )
        .await;
        assert!(result.is_ok(), "expected Ok, got: {:?}", result.err());

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            written.contains("report({ok = true})"),
            "file content missing expected lua: {written}"
        );
    }

    #[tokio::test]
    async fn generate_script_with_backend_write_error_propagates() {
        let result = generate_script_with_backend(
            args("test task", None, Some("/nonexistent/dir/output.lua")),
            valid_lua_backend(),
            None,
        )
        .await;
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("No such file or directory")
                || msg.contains("os error 2")
                || msg.contains("os error 3")
                || msg.contains("cannot find the path"),
            "expected filesystem error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn generate_script_with_backend_propagates_planner_error() {
        let null_backend: Arc<dyn AgentBackend> = Arc::new(MockBackend::new(
            "mock",
            vec![MockBehavior::Success {
                output: serde_json::Value::Null,
                tokens: TokenUsage::default(),
                delay: Duration::ZERO,
            }],
        ));

        let err = generate_script_with_backend(args("test task", None, None), null_backend, None)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("planner exhausted"),
            "expected planner exhausted error, got: {msg}"
        );
    }
}
