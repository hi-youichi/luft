use crate::install::types::{AgentType, McpConfigResult, Result};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// 获取用户主目录。
///
/// 优先使用 `USERPROFILE` / `HOME` 环境变量（可测试），
/// 回退到 `dirs::home_dir()`（Windows 上使用 `SHGetKnownFolderPath`）。
fn get_home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

/// Resolve the absolute path of the luft binary to embed in agent MCP configs.
///
/// Embedding the absolute path (rather than the bare `"luft"` command) means
/// agents don't depend on PATH resolution at launch time — the binary that
/// runs `luft install` is the exact binary the agent will spawn.
///
/// Resolution priority:
/// 1. `LUFT_BIN` env var — override for packagers / tests / scenarios where
///    `current_exe()` doesn't point at the canonical install location.
/// 2. `std::env::current_exe()` — the absolute path of the currently-running
///    luft binary.
/// 3. `"luft"` — bare command fallback (relies on PATH at agent-launch time),
///    used only if both above fail (extremely rare on any mainstream OS).
fn luft_command() -> String {
    if let Ok(path) = std::env::var("LUFT_BIN") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .unwrap_or_else(|| "luft".to_string())
}

/// MCP 配置器
///
/// 为 Claude、OpenCode、Codex 三种 Agent 配置 `luft mcp serve` MCP 服务器。
/// 每种 Agent 的配置文件格式和路径不同：
/// - Claude: `~/.claude/settings.json` (JSON, `mcpServers.luft`)
/// - OpenCode: `~/.config/opencode/opencode.json` 或 `.jsonc` (JSON/JSONC, `mcp.luft`)
/// - Codex: `~/.codex/config.toml` (TOML, `[mcp_servers.luft]`)
pub struct McpSetup;

impl McpSetup {
    /// 为检测到的 agents 配置 MCP 服务器。
    ///
    /// 每个 Agent 独立配置，一个失败不影响其他。
    /// 已存在 `luft` 条目时保留原有配置不覆盖。
    pub fn configure_for_agents(agents: &[AgentType]) -> Vec<(AgentType, McpConfigResult)> {
        let mut results = vec![];
        let mut processed = std::collections::HashSet::new();

        for agent in agents {
            if !processed.insert(agent.clone()) {
                continue;
            }
            let result = match agent {
                AgentType::Claude => Self::configure_claude(),
                AgentType::Opencode => Self::configure_opencode(),
                AgentType::Codex => Self::configure_codex(),
                _ => McpConfigResult::NotSupported,
            };
            results.push((agent.clone(), result));
        }
        results
    }

    // ── Claude ──────────────────────────────────────────────

    fn configure_claude() -> McpConfigResult {
        let config_dir = match Self::get_claude_config_dir() {
            Ok(dir) => dir,
            Err(e) => return McpConfigResult::Failed(e.to_string()),
        };
        let config_file = config_dir.join("settings.json");

        if let Err(e) = fs::create_dir_all(&config_dir) {
            return McpConfigResult::Failed(e.to_string());
        }

        let mut config = if config_file.exists() {
            match Self::read_json(&config_file) {
                Ok(v) => v,
                Err(e) => return McpConfigResult::Failed(e.to_string()),
            }
        } else {
            Value::Object(serde_json::Map::new())
        };

        if let Err(e) = Self::merge_claude_mcp(&mut config, &luft_command()) {
            return McpConfigResult::Failed(e.to_string());
        }

        if let Err(e) = Self::write_json(&config_file, &config) {
            return McpConfigResult::Failed(e.to_string());
        }

        McpConfigResult::Configured
    }

    fn merge_claude_mcp(config: &mut Value, luft_cmd: &str) -> Result<()> {
        let luft_entry = json!({
            "command": luft_cmd,
            "args": ["mcp", "serve"]
        });

        if let Some(obj) = config.as_object_mut() {
            if !obj.contains_key("mcpServers") {
                obj.insert("mcpServers".to_string(), json!({ "luft": luft_entry }));
            } else if let Some(servers) = obj
                .get_mut("mcpServers")
                .and_then(|v| v.as_object_mut())
            {
                if !servers.contains_key("luft") {
                    servers.insert("luft".to_string(), luft_entry);
                }
            }
        } else {
            let mut new_config = serde_json::Map::new();
            new_config.insert("mcpServers".to_string(), json!({ "luft": luft_entry }));
            *config = Value::Object(new_config);
        }

        Ok(())
    }

    fn get_claude_config_dir() -> Result<PathBuf> {
        let home = get_home_dir().ok_or(crate::install::types::InstallError::HomeDirNotFound)?;

        let possible_dirs = vec![
            home.join(".claude"),
            home.join("AppData").join("Roaming").join("claude"),
            home.join("Library")
                .join("Application Support")
                .join("claude"),
        ];

        for dir in &possible_dirs {
            if dir.exists() && dir.is_dir() {
                return Ok(dir.clone());
            }
        }

        Ok(home.join(".claude"))
    }

    // ── OpenCode ────────────────────────────────────────────

    fn configure_opencode() -> McpConfigResult {
        let config_dir = match Self::get_opencode_config_dir() {
            Ok(dir) => dir,
            Err(e) => return McpConfigResult::Failed(e.to_string()),
        };

        let jsonc_file = config_dir.join("opencode.jsonc");
        let json_file = config_dir.join("opencode.json");
        let (config_file, is_jsonc) = if jsonc_file.exists() {
            (jsonc_file, true)
        } else {
            (json_file, false)
        };

        if let Err(e) = fs::create_dir_all(&config_dir) {
            return McpConfigResult::Failed(e.to_string());
        }

        let mut config = if config_file.exists() {
            let content = match fs::read_to_string(&config_file) {
                Ok(c) => c,
                Err(e) => return McpConfigResult::Failed(e.to_string()),
            };
            let json_str = if is_jsonc {
                strip_jsonc_comments(&content)
            } else {
                content
            };
            match serde_json::from_str::<Value>(&json_str) {
                Ok(v) => v,
                Err(e) => return McpConfigResult::Failed(e.to_string()),
            }
        } else {
            Value::Object(serde_json::Map::new())
        };

        let luft_cmd = luft_command();
        let luft_entry = json!({
            "type": "local",
            "command": [luft_cmd, "mcp", "serve"]
        });

        if let Some(obj) = config.as_object_mut() {
            if !obj.contains_key("mcp") {
                obj.insert("mcp".to_string(), json!({}));
            }
            if let Some(mcp) = obj.get_mut("mcp").and_then(|v| v.as_object_mut()) {
                if !mcp.contains_key("luft") {
                    mcp.insert("luft".to_string(), luft_entry);
                }
            }
        } else {
            let mut new_config = serde_json::Map::new();
            new_config.insert("mcp".to_string(), json!({ "luft": luft_entry }));
            config = Value::Object(new_config);
        }

        if let Err(e) = Self::write_json(&config_file, &config) {
            return McpConfigResult::Failed(e.to_string());
        }

        McpConfigResult::Configured
    }

    fn get_opencode_config_dir() -> Result<PathBuf> {
        let home = get_home_dir().ok_or(crate::install::types::InstallError::HomeDirNotFound)?;
        Ok(home.join(".config").join("opencode"))
    }

    // ── Codex ───────────────────────────────────────────────

    fn configure_codex() -> McpConfigResult {
        let config_dir = match Self::get_codex_config_dir() {
            Ok(dir) => dir,
            Err(e) => return McpConfigResult::Failed(e.to_string()),
        };
        let config_file = config_dir.join("config.toml");

        if let Err(e) = fs::create_dir_all(&config_dir) {
            return McpConfigResult::Failed(e.to_string());
        }

        let content = if config_file.exists() {
            match fs::read_to_string(&config_file) {
                Ok(c) => c,
                Err(e) => return McpConfigResult::Failed(e.to_string()),
            }
        } else {
            String::new()
        };

        if content.contains("[mcp_servers.luft]") {
            return McpConfigResult::Configured;
        }

        let mut new_content = content;
        if !new_content.is_empty() && !new_content.ends_with('\n') {
            new_content.push('\n');
        }
        if !new_content.is_empty() {
            new_content.push('\n');
        }
        let luft_cmd = luft_command();
        new_content.push_str("[mcp_servers.luft]\n");
        // TOML literal string (single-quoted) so Windows backslashes in the
        // path need no escaping. Binary paths never contain single quotes.
        new_content.push_str(&format!("command = '{}'\n", luft_cmd));
        new_content.push_str("args = [\"mcp\", \"serve\"]\n");

        if let Err(e) = fs::write(&config_file, new_content) {
            return McpConfigResult::Failed(e.to_string());
        }

        McpConfigResult::Configured
    }

    fn get_codex_config_dir() -> Result<PathBuf> {
        let home = get_home_dir().ok_or(crate::install::types::InstallError::HomeDirNotFound)?;
        Ok(home.join(".codex"))
    }

    // ── 共享工具方法 ────────────────────────────────────────

    fn read_json(path: &Path) -> Result<Value> {
        let content = fs::read_to_string(path)?;
        let config: Value = serde_json::from_str(&content)?;
        Ok(config)
    }

    fn write_json(path: &Path, config: &Value) -> Result<()> {
        let content = serde_json::to_string_pretty(config)?;
        fs::write(path, content)?;
        Ok(())
    }
}

/// 去除 JSONC 注释（`//` 行注释和 `/* */` 块注释），保留字符串内部的注释字符。
fn strip_jsonc_comments(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escape = false;
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        if in_string {
            result.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
        } else if c == '"' {
            in_string = true;
            result.push(c);
            i += 1;
        } else if c == '/' && i + 1 < chars.len() {
            if chars[i + 1] == '/' {
                i += 2;
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            } else if chars[i + 1] == '*' {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i = (i + 2).min(chars.len());
            } else {
                result.push(c);
                i += 1;
            }
        } else {
            result.push(c);
            i += 1;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use serial_test::serial;
    use tempfile::TempDir;

    /// RAII guard: sets HOME + USERPROFILE to a temp dir, restores on drop.
    struct HomeGuard {
        home: Option<std::ffi::OsString>,
        userprofile: Option<std::ffi::OsString>,
    }

    impl HomeGuard {
        fn new(tmp: &Path) -> Self {
            let home = std::env::var_os("HOME");
            let userprofile = std::env::var_os("USERPROFILE");
            std::env::set_var("HOME", tmp);
            std::env::set_var("USERPROFILE", tmp);
            HomeGuard {
                home,
                userprofile,
            }
        }
    }

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match &self.home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
            match &self.userprofile {
                Some(v) => std::env::set_var("USERPROFILE", v),
                None => std::env::remove_var("USERPROFILE"),
            }
        }
    }

    /// RAII guard: sets `LUFT_BIN` to a fixed path for deterministic test
    /// assertions on the `command` value written to agent configs; restores
    /// (or removes) the previous value on drop. MUST be used inside `#[serial]`
    /// tests — `luft_command()` reads this env var.
    struct BinGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl BinGuard {
        fn new(path: &str) -> Self {
            let prev = std::env::var_os("LUFT_BIN");
            std::env::set_var("LUFT_BIN", path);
            BinGuard { prev }
        }
    }

    impl Drop for BinGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => std::env::set_var("LUFT_BIN", v),
                None => std::env::remove_var("LUFT_BIN"),
            }
        }
    }

    // ── Claude merge ────────────────────────────────────────

    #[test]
    fn merge_claude_mcp_into_empty() {
        let mut config = json!({});
        McpSetup::merge_claude_mcp(&mut config, "luft").unwrap();

        let servers = config["mcpServers"].as_object().unwrap();
        assert!(servers.contains_key("luft"));
        assert_eq!(servers["luft"]["command"], "luft");
        assert_eq!(servers["luft"]["args"], json!(["mcp", "serve"]));
    }

    #[test]
    fn merge_claude_mcp_preserves_existing() {
        let mut config = json!({
            "mcpServers": {
                "existing": {
                    "command": "existing-cmd",
                    "args": ["--existing"]
                }
            }
        });

        McpSetup::merge_claude_mcp(&mut config, "luft").unwrap();

        let servers = config["mcpServers"].as_object().unwrap();
        assert!(servers.contains_key("existing"));
        assert!(servers.contains_key("luft"));
        assert_eq!(servers["existing"]["command"], "existing-cmd");
    }

    #[test]
    fn merge_claude_mcp_no_duplicate() {
        let mut config = json!({
            "mcpServers": {
                "luft": {
                    "command": "old-luft",
                    "args": ["old-args"]
                }
            }
        });

        McpSetup::merge_claude_mcp(&mut config, "luft").unwrap();

        assert_eq!(config["mcpServers"]["luft"]["command"], "old-luft");
        assert_eq!(config["mcpServers"]["luft"]["args"], json!(["old-args"]));
    }

    #[test]
    fn merge_claude_mcp_uses_absolute_path_argument() {
        // The command value flows through verbatim — this is what lets
        // `configure_claude` pass `luft_command()` (an absolute path) and
        // have it reach disk unchanged.
        let mut config = json!({});
        McpSetup::merge_claude_mcp(&mut config, "/custom/path/to/luft").unwrap();
        assert_eq!(
            config["mcpServers"]["luft"]["command"],
            "/custom/path/to/luft"
        );
    }

    // ── OpenCode configure ─────────────────────────────────

    #[test]
    #[serial]
    fn configure_opencode_creates_new_json() {
        let tmp = TempDir::new().unwrap();
        let _guard = HomeGuard::new(tmp.path());
        let _bin = BinGuard::new("/test/luft");
        let dir = tmp.path().join(".config").join("opencode");

        let result = McpSetup::configure_for_agents(&[AgentType::Opencode]);
        assert_eq!(result[0].1, McpConfigResult::Configured);

        let config_file = dir.join("opencode.json");
        assert!(config_file.exists());

        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(&config_file).unwrap()).unwrap();
        assert_eq!(config["mcp"]["luft"]["type"], "local");
        assert_eq!(
            config["mcp"]["luft"]["command"],
            json!(["/test/luft", "mcp", "serve"])
        );
    }

    #[test]
    #[serial]
    fn configure_opencode_preserves_existing_mcp() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".config").join("opencode");
        fs::create_dir_all(&dir).unwrap();

        let existing = json!({
            "mcp": {
                "other": {
                    "type": "local",
                    "command": ["other-cmd"]
                }
            },
            "provider": {
                "test": {}
            }
        });
        fs::write(dir.join("opencode.json"), existing.to_string()).unwrap();

        let _guard = HomeGuard::new(tmp.path());
        let result = McpSetup::configure_for_agents(&[AgentType::Opencode]);
        assert_eq!(result[0].1, McpConfigResult::Configured);

        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("opencode.json")).unwrap())
                .unwrap();
        assert!(config["mcp"]["other"].is_object());
        assert!(config["mcp"]["luft"].is_object());
        assert!(config["provider"]["test"].is_object());
    }

    #[test]
    #[serial]
    fn configure_opencode_handles_jsonc() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".config").join("opencode");
        fs::create_dir_all(&dir).unwrap();

        let jsonc = r#"{
  // This is a comment
  "$schema": "https://opencode.ai/config.json",
  /* block comment */
  "provider": {
    "test": {} // inline comment
  }
}"#;
        fs::write(dir.join("opencode.jsonc"), jsonc).unwrap();

        let _guard = HomeGuard::new(tmp.path());
        let result = McpSetup::configure_for_agents(&[AgentType::Opencode]);
        assert_eq!(result[0].1, McpConfigResult::Configured);

        let content = std::fs::read_to_string(dir.join("opencode.jsonc")).unwrap();
        let config: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(config["$schema"], "https://opencode.ai/config.json");
        assert!(config["provider"]["test"].is_object());
        assert!(config["mcp"]["luft"].is_object());
    }

    #[test]
    #[serial]
    fn configure_opencode_preserves_existing_luft() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".config").join("opencode");
        fs::create_dir_all(&dir).unwrap();

        let existing = json!({
            "mcp": {
                "luft": {
                    "type": "local",
                    "command": ["custom-luft", "--backend", "codex"]
                }
            }
        });
        fs::write(dir.join("opencode.json"), existing.to_string()).unwrap();

        let _guard = HomeGuard::new(tmp.path());
        let result = McpSetup::configure_for_agents(&[AgentType::Opencode]);
        assert_eq!(result[0].1, McpConfigResult::Configured);

        let config: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("opencode.json")).unwrap())
                .unwrap();
        assert_eq!(
            config["mcp"]["luft"]["command"],
            json!(["custom-luft", "--backend", "codex"])
        );
    }

    // ── Codex configure ────────────────────────────────────

    #[test]
    #[serial]
    fn configure_codex_creates_new_toml() {
        let tmp = TempDir::new().unwrap();
        let _guard = HomeGuard::new(tmp.path());
        let _bin = BinGuard::new("/test/luft");
        let dir = tmp.path().join(".codex");

        let result = McpSetup::configure_for_agents(&[AgentType::Codex]);
        assert_eq!(result[0].1, McpConfigResult::Configured);

        let config_file = dir.join("config.toml");
        assert!(config_file.exists());

        let content = std::fs::read_to_string(&config_file).unwrap();
        assert!(content.contains("[mcp_servers.luft]"));
        // TOML literal string (single-quoted) — the absolute path flows
        // through verbatim, no backslash escaping.
        assert!(content.contains("command = '/test/luft'"));
        assert!(content.contains("args = [\"mcp\", \"serve\"]"));
    }

    #[test]
    #[serial]
    fn configure_codex_preserves_existing_content() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".codex");
        fs::create_dir_all(&dir).unwrap();

        let existing = r#"
model = "o4-mini"
approval_policy = "on-request"

[mcp_servers.docs]
command = "docs-server"
"#;
        fs::write(dir.join("config.toml"), existing).unwrap();

        let _guard = HomeGuard::new(tmp.path());
        let result = McpSetup::configure_for_agents(&[AgentType::Codex]);
        assert_eq!(result[0].1, McpConfigResult::Configured);

        let content = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(content.contains("model = \"o4-mini\""));
        assert!(content.contains("[mcp_servers.docs]"));
        assert!(content.contains("command = \"docs-server\""));
        assert!(content.contains("[mcp_servers.luft]"));
    }

    #[test]
    #[serial]
    fn configure_codex_preserves_existing_luft() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join(".codex");
        fs::create_dir_all(&dir).unwrap();

        let existing = r#"
[mcp_servers.luft]
command = "custom-luft"
args = ["custom-args"]
"#;
        fs::write(dir.join("config.toml"), existing).unwrap();

        let _guard = HomeGuard::new(tmp.path());
        let result = McpSetup::configure_for_agents(&[AgentType::Codex]);
        assert_eq!(result[0].1, McpConfigResult::Configured);

        let content = std::fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(content.contains("command = \"custom-luft\""));
        assert!(!content.contains("args = [\"mcp\", \"serve\"]"));
    }

    // ── configure_for_agents ───────────────────────────────

    #[test]
    #[serial]
    fn configure_for_agents_multiple() {
        let tmp = TempDir::new().unwrap();
        let _guard = HomeGuard::new(tmp.path());

        let results = McpSetup::configure_for_agents(&[
            AgentType::Claude,
            AgentType::Opencode,
            AgentType::Codex,
            AgentType::Mock,
        ]);

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].1, McpConfigResult::Configured); // Claude
        assert_eq!(results[1].1, McpConfigResult::Configured); // OpenCode
        assert_eq!(results[2].1, McpConfigResult::Configured); // Codex
        assert_eq!(results[3].1, McpConfigResult::NotSupported); // Mock
    }

    #[test]
    #[serial]
    fn configure_for_agents_dedup() {
        let tmp = TempDir::new().unwrap();
        let _guard = HomeGuard::new(tmp.path());

        let results =
            McpSetup::configure_for_agents(&[AgentType::Codex, AgentType::Codex, AgentType::Codex]);
        assert_eq!(results.len(), 1);
    }

    // ── strip_jsonc_comments ───────────────────────────────

    #[test]
    fn strip_line_comments() {
        let input = r#"{"a": 1 // comment
}"#;
        let stripped = strip_jsonc_comments(input);
        let parsed: Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn strip_block_comments() {
        let input = r#"{"a": /* comment */ 1}"#;
        let stripped = strip_jsonc_comments(input);
        let parsed: Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn strip_preserves_strings_with_slashes() {
        let input = r#"{"url": "https://example.com/path"}"#;
        let stripped = strip_jsonc_comments(input);
        let parsed: Value = serde_json::from_str(&stripped).unwrap();
        assert_eq!(parsed["url"], "https://example.com/path");
    }

    // ── read/write helpers ─────────────────────────────────

    #[test]
    fn merge_claude_mcp_round_trip() {
        let mut config = json!({});
        McpSetup::merge_claude_mcp(&mut config, "luft").unwrap();
        assert!(config["mcpServers"]["luft"].is_object());
        assert_eq!(config["mcpServers"]["luft"]["command"], "luft");

        // 二次合并不覆盖
        let original = config.clone();
        McpSetup::merge_claude_mcp(&mut config, "luft").unwrap();
        assert_eq!(config, original);
    }

    // ── McpConfigResult ────────────────────────────────────

    #[test]
    fn mcp_config_result_variants() {
        let configured = McpConfigResult::Configured;
        let not_supported = McpConfigResult::NotSupported;
        let failed = McpConfigResult::Failed("test error".to_string());

        assert_eq!(configured, McpConfigResult::Configured);
        assert_eq!(not_supported, McpConfigResult::NotSupported);
        assert!(matches!(failed, McpConfigResult::Failed(_)));
    }
}
