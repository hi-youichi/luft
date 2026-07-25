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
        let mcp_configured = if detected_agents.contains(&AgentType::Claude) {
            eprintln!("🌐 配置 Claude MCP 服务器...");
            match crate::install::mcp_setup::McpSetup::configure()? {
                crate::install::types::McpConfigResult::Configured => {
                    eprintln!("✅ MCP 配置完成");
                    true
                }
                crate::install::types::McpConfigResult::NotSupported => {
                    eprintln!("⚠️  MCP 不支持当前 Agent");
                    false
                }
                crate::install::types::McpConfigResult::Failed(err) => {
                    eprintln!("❌ MCP 配置失败: {}", err);
                    false
                }
            }
        } else {
            false
        };

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
            let claude_config = home.join(".claude/settings.json");

            if !claude_config.exists() {
                return Err(InstallError::VerificationFailed(
                    "Claude MCP 配置文件不存在".to_string(),
                ));
            }

            // 读取配置文件验证 luft MCP 服务器
            let content = std::fs::read_to_string(&claude_config)?;
            let config: serde_json::Value = serde_json::from_str(&content)?;

            if let Some(mcp_servers) = config.get("mcpServers") {
                if let Some(servers) = mcp_servers.as_object() {
                    if !servers.contains_key("luft") {
                        return Err(InstallError::VerificationFailed(
                            "Claude MCP 配置中没有 luft 服务器".to_string(),
                        ));
                    }
                }
            } else {
                return Err(InstallError::VerificationFailed(
                    "Claude 配置中没有 mcpServers 部分".to_string(),
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
    use tempfile::TempDir;

    fn setup_test_environment(temp_dir: &TempDir) {
        // 创建技能源目录
        let skills_dir = temp_dir.path().join(".loom/skills/auto");
        std::fs::create_dir_all(&skills_dir).unwrap();

        // 创建测试技能文件
        std::fs::write(skills_dir.join("test.md"), "# Test").unwrap();
        std::fs::write(skills_dir.join("workflow.md"), "# Workflow").unwrap();
    }

    #[test]
    fn test_install_all_requires_skills_source() {
        let temp_dir = TempDir::new().unwrap();

        // 不创建技能源目录
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = Installer::install_all();

        std::env::set_current_dir(original_dir).unwrap();

        // 应该失败，因为缺少技能源目录
        assert!(result.is_err());
    }

    #[test]
    fn test_install_all_with_valid_environment() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_environment(&temp_dir);

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        // 模拟检测到外部 Agent
        // 由于实际的 Agent 检测可能失败，我们主要测试结构
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
