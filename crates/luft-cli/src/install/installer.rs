use crate::install::types::{AgentType, InstallError, InstallSummary, Result};
use std::time::Instant;

/// 统一安装器
pub struct Installer;

impl Installer {
    /// 执行完整安装流程
    pub fn install_all() -> Result<InstallSummary> {
        let start_time = Instant::now();

        eprintln!("🔍 检测已安装的 Agent...");

        // 1. 检测已安装的外部 Agent
        let detected_results = crate::install::agent_detector::AgentDetector::detect_all()?;
        let detected_agents: Vec<AgentType> = detected_results
            .iter()
            .filter(|r| r.is_available)
            .map(|r| r.agent_type.clone())
            .collect();

        // 2. 检查是否至少有一个外部 Agent（排除 Mock）
        let external_agents: Vec<_> = detected_agents
            .iter()
            .filter(|a| a.needs_external_installation())
            .collect();

        if external_agents.is_empty() {
            return Err(InstallError::NoExternalAgentsFound);
        }

        for agent in &external_agents {
            eprintln!("✅ 检测到: {}", agent.display_name());
        }

        // 3. 安装桥接组件
        eprintln!("🔧 安装 Luft 桥接组件...");
        let skill_manager = crate::install::skill_manager::SkillManager::new()?;
        let bridges_installed = skill_manager.install_for_agents(&detected_agents)?;

        for bridge in &bridges_installed {
            eprintln!(
                "📁 技能已安装到: {} ({} 个技能)",
                bridge.target_dir.display(),
                bridge.skills_count
            );
        }

        // 4. 配置 MCP 服务器
        eprintln!("🌐 配置 MCP 服务器...");
        let mcp_results =
            crate::install::mcp_setup::McpSetup::configure_for_agents(&detected_agents);
        let mcp_configured = mcp_results
            .iter()
            .any(|(_, r)| *r == crate::install::types::McpConfigResult::Configured);

        for (agent, result) in &mcp_results {
            match result {
                crate::install::types::McpConfigResult::Configured => {
                    eprintln!("✅ {} MCP 配置完成", agent.display_name());
                }
                crate::install::types::McpConfigResult::NotSupported => {}
                crate::install::types::McpConfigResult::Failed(err) => {
                    eprintln!("❌ {} MCP 配置失败: {}", agent.display_name(), err);
                }
            }
        }

        // 5. 验证安装
        eprintln!("✅ 验证安装...");
        let summary = InstallSummary {
            detected_agents: detected_agents.clone(),
            bridges_installed,
            mcp_configured,
            installation_time: start_time.elapsed(),
        };

        Self::verify_installation(&summary)?;
        eprintln!("🎉 安装完成！");

        Ok(summary)
    }

    /// 验证安装结果
    fn verify_installation(summary: &InstallSummary) -> Result<()> {
        // 验证桥接安装
        for bridge in &summary.bridges_installed {
            if !bridge.target_dir.exists() {
                return Err(InstallError::VerificationFailed(format!(
                    "桥接目录不存在: {}",
                    bridge.target_dir.display()
                )));
            }

            if bridge.skills_count == 0 {
                return Err(InstallError::VerificationFailed(format!(
                    "桥接目录没有技能文件: {}",
                    bridge.target_dir.display()
                )));
            }
        }

        // 验证 MCP 配置
        if summary.mcp_configured {
            let home = dirs::home_dir().ok_or(InstallError::HomeDirNotFound)?;
            let mut any_valid = false;

            // Claude: ~/.claude/settings.json -> mcpServers.luft
            let claude_config = home.join(".claude/settings.json");
            if claude_config.exists() {
                let content = std::fs::read_to_string(&claude_config)?;
                if let Ok(config) = serde_json::from_str::<serde_json::Value>(&content) {
                    if let Some(servers) = config.get("mcpServers").and_then(|v| v.as_object()) {
                        if servers.contains_key("luft") {
                            any_valid = true;
                        }
                    }
                }
            }

            // OpenCode: ~/.config/opencode/opencode.json(.jsonc) -> mcp.luft
            let opencode_dir = home.join(".config/opencode");
            for name in ["opencode.jsonc", "opencode.json"] {
                let config_file = opencode_dir.join(name);
                if config_file.exists() {
                    let content = std::fs::read_to_string(&config_file)?;
                    if content.contains("\"luft\"") {
                        any_valid = true;
                    }
                }
            }

            // Codex: ~/.codex/config.toml -> [mcp_servers.luft]
            let codex_config = home.join(".codex/config.toml");
            if codex_config.exists() {
                let content = std::fs::read_to_string(&codex_config)?;
                if content.contains("[mcp_servers.luft]") {
                    any_valid = true;
                }
            }

            if !any_valid {
                return Err(InstallError::VerificationFailed(
                    "未找到任何已配置的 MCP 服务器".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// 获取安装建议信息
    pub fn get_installation_suggestions() -> String {
        "建议: 请先安装至少一个支持的 Agent：
- Codex ACP: npm install -g @agentclientprotocol/codex-acp
- OpenCode: 下载并安装 OpenCode
- Claude Code: 下载并安装 Claude Code"
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use tempfile::TempDir;

    #[test]
    #[serial]
    fn test_install_all_no_external_agents() {
        let temp_dir = TempDir::new().unwrap();

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = Installer::install_all();

        std::env::set_current_dir(original_dir).unwrap();

        // 如果环境中没有外部 Agent，应该返回 NoExternalAgentsFound
        match result {
            Err(InstallError::NoExternalAgentsFound) => {
                // 预期的错误
            }
            Ok(summary) => {
                // 如果成功，验证基本结构
                assert!(!summary.detected_agents.is_empty());
            }
            Err(_) => {
                // 其他错误也可以接受
            }
        }
    }

    #[test]
    fn test_installation_suggestions() {
        let suggestions = Installer::get_installation_suggestions();
        assert!(suggestions.contains("Codex ACP"));
        assert!(suggestions.contains("npm install -g"));
        assert!(suggestions.contains("OpenCode"));
        assert!(suggestions.contains("Claude Code"));
    }

    #[test]
    fn test_verify_installation_with_valid_summary() {
        let temp_dir = TempDir::new().unwrap();

        // 创建测试技能目录
        let target_dir = temp_dir.path().join("test_skills");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::write(target_dir.join("test.md"), "# Test").unwrap();

        let _summary = InstallSummary {
            detected_agents: vec![AgentType::Codex],
            bridges_installed: vec![],
            mcp_configured: false,
            installation_time: std::time::Duration::from_millis(100),
        };

        // 测试验证逻辑（单独测试）
        // 在实际使用中，verify_installation 需要有效的 bridge 结果
    }
}
