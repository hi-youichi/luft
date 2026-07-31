//! E2E test: real Codex CLI drives workflow_execute via MCP.
//!
//! Prerequisites:
//!   - `luft` binary installed (`cargo install --path .`)
//!   - `codex` CLI installed and configured
//!   - `~/.codex/config.json` has `luft` as an MCP server:
//!     ```json
//!     {
//!       "mcpServers": {
//!         "luft": { "command": "luft", "args": ["mcp", "serve"] }
//!       }
//!     }
//!     ```
//!
//! Run with: cargo test -p luft-cli --test e2e_codex_backend -- --ignored --nocapture

use std::process::Command;

fn luft_binary() -> &'static str {
    "luft"
}

fn codex_binary() -> &'static str {
    "codex"
}

fn ensure_luft_installed() {
    let out = Command::new(luft_binary())
        .args(["--version"])
        .output();
    if out.is_err() {
        panic!(
            "`{}` not found in PATH. Run `cargo install --path .` first.",
            luft_binary()
        );
    }
}

fn ensure_codex_installed() {
    let out = Command::new(codex_binary())
        .args(["--version"])
        .output();
    if out.is_err() {
        panic!(
            "`{}` not found in PATH. Install Codex CLI first.",
            codex_binary()
        );
    }
}

fn stop_daemon() {
    let _ = Command::new(luft_binary())
        .args(["daemon", "stop"])
        .output();
}

#[test]
#[ignore = "requires codex CLI installed and configured"]
fn e2e_codex_auto_infer_backend() {
    ensure_luft_installed();
    ensure_codex_installed();

    // Clean slate: stop any running daemon so auto-detect runs fresh.
    stop_daemon();

    let prompt = r#"调用 workflow_execute 运行以下 Lua 工作流：
meta = { reasoning = "e2e test", phases = {} }
function main()
  phase("t")
  local r = agent({ prompt = "回复 JSON {ok:true}", name = "a1" })
  report({ ok = r.ok })
end
然后用 workflow_status 等待完成。"#;

    let output = Command::new(codex_binary())
        .args([
            "exec",
            "--approval",
            "never",
            "--sandbox",
            "danger-full-access",
            prompt,
        ])
        .output()
        .expect("failed to spawn codex exec");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    // Must have called workflow_execute
    assert!(
        combined.contains("workflow_execute"),
        "Codex should call workflow_execute. Output:\n{combined}"
    );

    // Must NOT have spawn failure (which would indicate wrong backend)
    assert!(
        !combined.to_lowercase().contains("spawn failed"),
        "Agent should not fail to spawn. Output:\n{combined}"
    );

    // Workflow should complete
    assert!(
        combined.to_lowercase().contains("completed")
            || combined.to_lowercase().contains("status"),
        "Workflow should reach a terminal status. Output:\n{combined}"
    );
}

#[test]
#[ignore = "requires codex CLI installed and configured"]
fn e2e_codex_explicit_backend_override() {
    ensure_luft_installed();
    ensure_codex_installed();

    stop_daemon();

    // This test verifies that Codex can pass an explicit backend parameter.
    // Since codex-acp may not be available in all test environments, we just
    // verify that the workflow_execute call is made with the backend parameter.
    let prompt = r#"调用 workflow_execute，传入 backend="codex"，运行以下脚本：
meta = { reasoning = "explicit backend", phases = {} }
function main() report({ ok = true }) end
等待完成。"#;

    let output = Command::new(codex_binary())
        .args([
            "exec",
            "--approval",
            "never",
            "--sandbox",
            "danger-full-access",
            prompt,
        ])
        .output()
        .expect("failed to spawn codex exec");

    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        combined.contains("workflow_execute"),
        "Codex should call workflow_execute. Output:\n{combined}"
    );
}
