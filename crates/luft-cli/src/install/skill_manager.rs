use crate::install::types::{AgentType, BridgeInstallResult, Result};
use dirs::home_dir;
use std::path::{Path, PathBuf};

/// 技能管理器
///
/// 从编译内置的 `luft_skills::WORKFLOW_SKILL` 写入技能文件到各 Agent 目录，
/// 不再依赖运行时 `.loom/skills/auto/` 目录。
pub struct SkillManager;

impl SkillManager {
    /// 创建技能管理器
    pub fn new() -> Result<Self> {
        Ok(Self)
    }

    /// 为指定的 Agent 类型安装技能
    pub fn install_for_agents(&self, agents: &[AgentType]) -> Result<Vec<BridgeInstallResult>> {
        let mut results = vec![];
        let mut processed_codex_opencode = false;
        let mut processed_claude = false;

        for agent in agents {
            match agent {
                AgentType::Mock => continue,

                AgentType::Codex | AgentType::Opencode => {
                    if !processed_codex_opencode {
                        let target_dir = home_dir()
                            .ok_or(crate::install::types::InstallError::HomeDirNotFound)?
                            .join(".agents/skills/workflow");

                        let skills_count = self.write_skills_to(&target_dir)?;
                        results.push(BridgeInstallResult {
                            agent_type: vec![AgentType::Codex, AgentType::Opencode],
                            target_dir: target_dir.clone(),
                            skills_count,
                        });
                        processed_codex_opencode = true;
                    }
                }

                AgentType::Claude => {
                    if !processed_claude {
                        let target_dir = home_dir()
                            .ok_or(crate::install::types::InstallError::HomeDirNotFound)?
                            .join(".claude/skills/workflow");

                        let skills_count = self.write_skills_to(&target_dir)?;
                        results.push(BridgeInstallResult {
                            agent_type: vec![AgentType::Claude],
                            target_dir: target_dir.clone(),
                            skills_count,
                        });
                        processed_claude = true;
                    }
                }

                AgentType::Hermes => {
                    let target_dir = home_dir()
                        .ok_or(crate::install::types::InstallError::HomeDirNotFound)?
                        .join(".hermes/skills/luft/workflow");

                    let skills_count = self.write_skills_to(&target_dir)?;
                    results.push(BridgeInstallResult {
                        agent_type: vec![AgentType::Hermes],
                        target_dir: target_dir.clone(),
                        skills_count,
                    });
                }

                AgentType::Custom(id) => {
                    let target_dir = home_dir()
                        .ok_or(crate::install::types::InstallError::HomeDirNotFound)?
                        .join(format!(".{}/skills/workflow", id));

                    let skills_count = self.write_skills_to(&target_dir)?;
                    results.push(BridgeInstallResult {
                        agent_type: vec![AgentType::Custom(id.clone())],
                        target_dir: target_dir.clone(),
                        skills_count,
                    });
                }

                AgentType::FutureExtension => {
                    return Err(crate::install::types::InstallError::UnsupportedAgentType);
                }
            }
        }

        Ok(results)
    }

    /// 将编译内置的 workflow 技能写入目标目录
    pub fn write_skills_to(&self, target_dir: &Path) -> Result<usize> {
        let count = luft_skills::write_to_dir(target_dir, &luft_skills::WORKFLOW_SKILL)?;
        Ok(count)
    }

    /// 获取目标目录列表
    #[allow(dead_code)]
    fn get_target_directories(&self, agents: &[AgentType]) -> Result<Vec<PathBuf>> {
        let mut dirs = vec![];
        let home = home_dir().ok_or(crate::install::types::InstallError::HomeDirNotFound)?;

        for agent in agents {
            match agent {
                AgentType::Codex | AgentType::Opencode => {
                    let dir = home.join(".agents/skills/workflow");
                    if !dirs.contains(&dir) {
                        dirs.push(dir);
                    }
                }

                AgentType::Claude => {
                    let dir = home.join(".claude/skills/workflow");
                    if !dirs.contains(&dir) {
                        dirs.push(dir);
                    }
                }

                AgentType::Hermes => {
                    let dir = home.join(".hermes/skills/luft/workflow");
                    if !dirs.contains(&dir) {
                        dirs.push(dir);
                    }
                }

                AgentType::Custom(id) => {
                    let dir = home.join(format!(".{}/skills/workflow", id));
                    if !dirs.contains(&dir) {
                        dirs.push(dir);
                    }
                }

                AgentType::Mock | AgentType::FutureExtension => continue,
            }
        }

        Ok(dirs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_skill_manager_creation() {
        let result = SkillManager::new();
        assert!(result.is_ok());
    }

    #[test]
    fn test_write_skills_to_directory() {
        let temp_dir = TempDir::new().unwrap();
        let target_dir = temp_dir.path().join("target_skills");

        let skill_manager = SkillManager::new().unwrap();
        let count = skill_manager.write_skills_to(&target_dir).unwrap();

        // 1 SKILL.md + 6 references
        assert_eq!(count, 7);
        assert!(target_dir.join("SKILL.md").exists());
        assert!(target_dir.join("references/primitives.md").exists());
        assert!(target_dir.join("references/examples.md").exists());
    }

    #[test]
    fn test_install_for_codex() {
        let temp_dir = TempDir::new().unwrap();
        let original_home = home_dir();
        std::env::set_var("HOME", temp_dir.path());

        let skill_manager = SkillManager::new().unwrap();
        let results = skill_manager
            .install_for_agents(&[AgentType::Codex])
            .unwrap();

        std::env::remove_var("HOME");
        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].skills_count, 7);
        assert!(results[0].agent_type.contains(&AgentType::Codex));
    }

    #[test]
    fn test_install_for_claude() {
        let temp_dir = TempDir::new().unwrap();
        let original_home = home_dir();
        std::env::set_var("HOME", temp_dir.path());

        let skill_manager = SkillManager::new().unwrap();
        let results = skill_manager
            .install_for_agents(&[AgentType::Claude])
            .unwrap();

        std::env::remove_var("HOME");
        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].skills_count, 7);
        assert!(results[0].agent_type.contains(&AgentType::Claude));
    }

    #[test]
    fn test_install_for_multiple_agents() {
        let temp_dir = TempDir::new().unwrap();
        let original_home = home_dir();
        std::env::set_var("HOME", temp_dir.path());

        let skill_manager = SkillManager::new().unwrap();
        let results = skill_manager
            .install_for_agents(&[AgentType::Codex, AgentType::Claude])
            .unwrap();

        std::env::remove_var("HOME");
        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_install_for_hermes() {
        let temp_dir = TempDir::new().unwrap();
        let original_home = home_dir();
        std::env::set_var("HOME", temp_dir.path());

        let skill_manager = SkillManager::new().unwrap();
        let results = skill_manager
            .install_for_agents(&[AgentType::Hermes])
            .unwrap();

        std::env::remove_var("HOME");
        if let Some(h) = original_home {
            std::env::set_var("HOME", h);
        }

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].skills_count, 7);
        assert!(results[0].agent_type.contains(&AgentType::Hermes));
        assert!(results[0].target_dir.ends_with(".hermes/skills/luft/workflow"));
    }
}
