use crate::install::types::{McpConfigResult, Result};
use dirs::home_dir;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

/// MCP 配置器
pub struct McpSetup;

impl McpSetup {
    /// 配置 Claude MCP 服务器
    pub fn configure() -> Result<McpConfigResult> {
        let config_dir = Self::get_claude_config_dir()?;
        let config_file = config_dir.join("settings.json");

        // 创建配置目录
        fs::create_dir_all(&config_dir)?;

        // 读取或创建配置
        let mut config = if config_file.exists() {
            Self::read_config(&config_file)?
        } else {
            Value::Object(serde_json::Map::new())
        };

        // 合并 MCP 配置
        Self::merge_mcp_config(&mut config)?;

        // 写回文件
        Self::write_config(&config_file, &config)?;

        Ok(McpConfigResult::Configured)
    }

    /// 合并 MCP 配置到现有配置
    fn merge_mcp_config(config: &mut Value) -> Result<()> {
        let mcp_servers = json!({
            "luft": {
                "command": "luft",
                "args": ["mcp", "serve"]
            }
        });

        if let Some(obj) = config.as_object_mut() {
            if !obj.contains_key("mcpServers") {
                obj.insert("mcpServers".to_string(), mcp_servers);
            } else if let Some(servers) = obj.get_mut("mcpServers").and_then(|v| v.as_object_mut())
            {
                if !servers.contains_key("luft") {
                    servers.insert("luft".to_string(), mcp_servers["luft"].clone());
                }
                // 如果已存在 luft，保留原有配置
            }
        } else {
            // 如果 config 不是对象，创建新的
            let mut new_config = serde_json::Map::new();
            new_config.insert("mcpServers".to_string(), mcp_servers);
            *config = Value::Object(new_config);
        }

        Ok(())
    }

    /// 读取配置文件
    fn read_config(path: &Path) -> Result<Value> {
        let content = fs::read_to_string(path)?;
        let config: Value = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// 写入配置文件
    fn write_config(path: &Path, config: &Value) -> Result<()> {
        let content = serde_json::to_string_pretty(config)?;
        fs::write(path, content)?;
        Ok(())
    }

    /// 获取 Claude 配置目录
    fn get_claude_config_dir() -> Result<PathBuf> {
        let home = home_dir().ok_or(crate::install::types::InstallError::HomeDirNotFound)?;

        // 检查常见的 Claude 配置目录
        let possible_dirs = vec![
            home.join(".claude"),
            home.join("AppData").join("Roaming").join("claude"),
            home.join("Library")
                .join("Application Support")
                .join("claude"),
        ];

        // 返回第一个存在的目录，否则使用默认的 ~/.claude
        for dir in &possible_dirs {
            if dir.exists() && dir.is_dir() {
                return Ok(dir.clone());
            }
        }

        // 如果都不存在，使用默认的 ~/.claude
        Ok(home.join(".claude"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    #[test]
    fn test_merge_mcp_config_into_empty() {
        let mut config = json!({});
        McpSetup::merge_mcp_config(&mut config).unwrap();

        assert!(config.is_object());
        assert!(config.get("mcpServers").is_some());

        let mcp_servers = config["mcpServers"].as_object().unwrap();
        assert!(mcp_servers.contains_key("luft"));
        assert_eq!(mcp_servers["luft"]["command"], "luft");
    }

    #[test]
    fn test_merge_mcp_config_preserves_existing() {
        let mut config = json!({
            "mcpServers": {
                "existing": {
                    "command": "existing-cmd",
                    "args": ["--existing"]
                }
            }
        });

        McpSetup::merge_mcp_config(&mut config).unwrap();

        let servers = config["mcpServers"].as_object().unwrap();
        assert!(servers.contains_key("existing"));
        assert!(servers.contains_key("luft"));
        assert_eq!(servers["existing"]["command"], "existing-cmd");
    }

    #[test]
    fn test_merge_mcp_config_no_duplicate() {
        let mut config = json!({
            "mcpServers": {
                "luft": {
                    "command": "old-luft",
                    "args": ["old-args"]
                }
            }
        });

        McpSetup::merge_mcp_config(&mut config).unwrap();

        assert_eq!(config["mcpServers"]["luft"]["command"], "old-luft");
        // 保留原有的配置
        assert_eq!(config["mcpServers"]["luft"]["args"], json!(["old-args"]));
    }

    #[test]
    fn test_write_and_read_config() {
        let temp_dir = TempDir::new().unwrap();
        let config_file = temp_dir.path().join("settings.json");

        let test_config = json!({
            "mcpServers": {
                "test": {
                    "command": "test-cmd"
                }
            }
        });

        McpSetup::write_config(&config_file, &test_config).unwrap();
        assert!(config_file.exists());

        let read_config = McpSetup::read_config(&config_file).unwrap();
        assert_eq!(read_config, test_config);
    }

    #[test]
    fn test_configure_creates_new_file() {
        let temp_dir = TempDir::new().unwrap();
        let _original_home = home_dir().unwrap();

        // 创建临时 .claude 目录
        let temp_claude_dir = temp_dir.path().join(".claude");
        fs::create_dir_all(&temp_claude_dir).unwrap();

        // 临时修改环境变量或使用模拟
        // 这里我们直接测试 merge_mcp_config 和 write/read 功能

        let config_file = temp_claude_dir.join("settings.json");
        assert!(!config_file.exists());

        let mut config = json!({});
        McpSetup::merge_mcp_config(&mut config).unwrap();
        McpSetup::write_config(&config_file, &config).unwrap();

        assert!(config_file.exists());

        let read_config = McpSetup::read_config(&config_file).unwrap();
        assert!(read_config["mcpServers"]["luft"].is_object());
    }

    #[test]
    fn test_mcp_config_result() {
        let configured = McpConfigResult::Configured;
        let not_supported = McpConfigResult::NotSupported;
        let failed = McpConfigResult::Failed("test error".to_string());

        assert_eq!(configured, McpConfigResult::Configured);
        assert_eq!(not_supported, McpConfigResult::NotSupported);
        assert!(matches!(failed, McpConfigResult::Failed(_)));
    }
}
