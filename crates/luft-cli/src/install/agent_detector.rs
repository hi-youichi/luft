use crate::install::types::{AgentDetectionResult, AgentType, Result};
use dirs::{config_dir, data_local_dir, home_dir};
use std::path::PathBuf;
use std::process::Command;

/// Agent 检测器
pub struct AgentDetector;

impl AgentDetector {
    /// 检测所有可用的 Agent
    pub fn detect_all() -> Result<Vec<AgentDetectionResult>> {
        let mut results = vec![];

        // Mock 总是可用
        results.push(AgentDetectionResult {
            agent_type: AgentType::Mock,
            is_available: true,
            skill_directory: PathBuf::new(), // Mock 不需要技能目录
            supports_mcp: false,
            detector_name: "MockDetector".to_string(),
        });

        // 检测 Codex ACP
        let codex_available = Self::is_codex_acp_installed()?;
        if codex_available {
            results.push(AgentDetectionResult {
                agent_type: AgentType::Codex,
                is_available: true,
                skill_directory: Self::get_codex_skill_directory()?,
                supports_mcp: true,
                detector_name: "CodexDetector".to_string(),
            });
        }

        // 检测 OpenCode
        let opencode_available = Self::is_opencode_installed()?;
        if opencode_available {
            results.push(AgentDetectionResult {
                agent_type: AgentType::Opencode,
                is_available: true,
                skill_directory: Self::get_opencode_skill_directory()?,
                supports_mcp: true,
                detector_name: "OpenCodeDetector".to_string(),
            });
        }

        // 检测 Claude Code
        let claude_available = Self::is_claude_code_installed()?;
        if claude_available {
            results.push(AgentDetectionResult {
                agent_type: AgentType::Claude,
                is_available: true,
                skill_directory: Self::get_claude_skill_directory()?,
                supports_mcp: true,
                detector_name: "ClaudeDetector".to_string(),
            });
        }

        // 检测 Hermes Agent
        let hermes_available = Self::is_hermes_installed()?;
        if hermes_available {
            results.push(AgentDetectionResult {
                agent_type: AgentType::Hermes,
                is_available: true,
                skill_directory: Self::get_hermes_skill_directory()?,
                supports_mcp: true,
                detector_name: "HermesDetector".to_string(),
            });
        }

        Ok(results)
    }

    /// 检测 codex-acp 是否已安装
    fn is_codex_acp_installed() -> Result<bool> {
        // 检查 PATH 上的 codex 二进制
        if which::which("codex").is_ok() {
            return Ok(true);
        }

        // 检查 ~/.codex 配置目录（Codex Desktop / CLI 安装后存在）
        if let Some(home) = home_dir() {
            if home.join(".codex").exists() {
                return Ok(true);
            }
        }

        // 检查 Codex Desktop 常见安装路径
        for path in Self::get_codex_paths() {
            if path.exists() {
                return Ok(true);
            }
        }

        // 检查 npm 全局安装
        if Self::check_npm_global("@agentclientprotocol/codex-acp")? {
            return Ok(true);
        }

        // 检查 npx 可用性
        if Self::check_npx_codex()? {
            return Ok(true);
        }

        Ok(false)
    }

    /// 检测 opencode 是否已安装
    fn is_opencode_installed() -> Result<bool> {
        // 检查命令存在
        if which::which("opencode").is_ok() {
            return Ok(true);
        }

        // 检查常见路径
        let opencode_paths = Self::get_opencode_paths();
        for path in opencode_paths {
            if path.exists() {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// 检测 claude code 是否已安装
    fn is_claude_code_installed() -> Result<bool> {
        // 检查命令存在
        if which::which("claude").is_ok() {
            return Ok(true);
        }

        // 检查配置目录存在
        let claude_paths = Self::get_claude_paths();
        for path in claude_paths {
            if path.exists() && path.is_dir() {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// 检测 Hermes Agent 是否已安装
    fn is_hermes_installed() -> Result<bool> {
        // 检查命令存在
        if which::which("hermes").is_ok() {
            return Ok(true);
        }

        // 检查 ~/.hermes 配置目录
        if let Some(home) = home_dir() {
            if home.join(".hermes").exists() {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// 检查 npm 全局安装
    fn check_npm_global(package: &str) -> Result<bool> {
        let output = Command::new("npm").args(["list", "-g", package]).output();

        match output {
            Ok(result) => {
                if result.status.success() {
                    let output_str = String::from_utf8_lossy(&result.stdout);
                    Ok(!output_str.contains("empty"))
                } else {
                    Ok(false)
                }
            }
            Err(_) => Ok(false),
        }
    }

    /// 检查 npx codex-acp 可用性
    fn check_npx_codex() -> Result<bool> {
        let output = Command::new("npx")
            .args(["-y", "@agentclientprotocol/codex-acp", "--version"])
            .output();

        match output {
            Ok(result) => Ok(result.status.success()),
            Err(_) => Ok(false),
        }
    }

    /// 获取 Codex 技能目录
    fn get_codex_skill_directory() -> Result<PathBuf> {
        let home = home_dir().ok_or(crate::install::types::InstallError::HomeDirNotFound)?;
        Ok(home.join(".agents/skills/workflow"))
    }

    /// 获取 Codex Desktop / CLI 常见路径
    fn get_codex_paths() -> Vec<PathBuf> {
        let mut paths = vec![];

        if let Some(home) = home_dir() {
            paths.push(home.join(".codex"));
        }

        if let Some(data_local) = data_local_dir() {
            // Windows: %LOCALAPPDATA%\OpenAI\Codex
            paths.push(data_local.join("OpenAI").join("Codex").join("bin"));
        }

        paths
    }

    /// 获取 OpenCode 技能目录
    fn get_opencode_skill_directory() -> Result<PathBuf> {
        let home = home_dir().ok_or(crate::install::types::InstallError::HomeDirNotFound)?;
        Ok(home.join(".agents/skills/workflow"))
    }

    /// 获取 Claude Code 技能目录
    fn get_claude_skill_directory() -> Result<PathBuf> {
        let home = home_dir().ok_or(crate::install::types::InstallError::HomeDirNotFound)?;
        Ok(home.join(".claude/skills/workflow"))
    }

    /// 获取 Hermes Agent 技能目录
    fn get_hermes_skill_directory() -> Result<PathBuf> {
        let home = home_dir().ok_or(crate::install::types::InstallError::HomeDirNotFound)?;
        Ok(home.join(".hermes/skills/luft/workflow"))
    }

    /// 获取 OpenCode 常见路径
    fn get_opencode_paths() -> Vec<PathBuf> {
        let mut paths = vec![];

        if let Some(home) = home_dir() {
            paths.push(home.join(".opencode"));
        }

        if let Some(config) = config_dir() {
            paths.push(config.join("opencode"));
        }

        if let Some(data_local) = data_local_dir() {
            paths.push(data_local.join("opencode"));
        }

        paths
    }

    /// 获取 Claude Code 常见路径
    fn get_claude_paths() -> Vec<PathBuf> {
        let mut paths = vec![];

        if let Some(home) = home_dir() {
            paths.push(home.join(".claude"));
        }

        if let Some(config) = config_dir() {
            paths.push(config.join("claude"));
        }

        if let Some(data_local) = data_local_dir() {
            paths.push(data_local.join("claude"));
        }

        paths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_all_returns_at_least_mock() {
        let results = AgentDetector::detect_all().unwrap();
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.agent_type == AgentType::Mock));
    }

    #[test]
    fn test_detect_codex_acp() {
        let result = AgentDetector::is_codex_acp_installed();
        // 根据实际环境，结果可能是 true 或 false
        assert!(result.is_ok());
    }

    #[test]
    fn test_detect_opencode() {
        let result = AgentDetector::is_opencode_installed();
        assert!(result.is_ok());
    }

    #[test]
    fn test_detect_claude_code() {
        let result = AgentDetector::is_claude_code_installed();
        assert!(result.is_ok());
    }

    #[test]
    fn test_detect_hermes() {
        let result = AgentDetector::is_hermes_installed();
        assert!(result.is_ok());
    }

    #[test]
    fn test_skill_directories() {
        let codex_dir = AgentDetector::get_codex_skill_directory();
        assert!(codex_dir.is_ok());
        assert!(codex_dir.unwrap().ends_with(".agents/skills/workflow"));

        let claude_dir = AgentDetector::get_claude_skill_directory();
        assert!(claude_dir.is_ok());
        assert!(claude_dir.unwrap().ends_with(".claude/skills/workflow"));
    }
}
