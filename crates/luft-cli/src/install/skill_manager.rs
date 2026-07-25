use crate::install::types::{AgentType, BridgeInstallResult, Result};
use dirs::home_dir;
use std::fs;
use std::path::{Path, PathBuf};

/// 技能管理器
pub struct SkillManager {
    source_dir: PathBuf,
}

impl SkillManager {
    /// 创建技能管理器
    pub fn new() -> Result<Self> {
        let source_dir = PathBuf::from(".loom/skills/auto");

        if !source_dir.exists() {
            return Err(crate::install::types::InstallError::SkillSourceNotFound(
                source_dir,
            ));
        }

        Ok(Self { source_dir })
    }

    /// 为指定的 Agent 类型安装技能
    pub fn install_for_agents(&self, agents: &[AgentType]) -> Result<Vec<BridgeInstallResult>> {
        let _target_dirs = self.get_target_directories(agents)?;

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

                        let skills_count = self.copy_skills_to(&target_dir)?;
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

                        let skills_count = self.copy_skills_to(&target_dir)?;
                        results.push(BridgeInstallResult {
                            agent_type: vec![AgentType::Claude],
                            target_dir: target_dir.clone(),
                            skills_count,
                        });
                        processed_claude = true;
                    }
                }

                AgentType::Custom(id) => {
                    let target_dir = home_dir()
                        .ok_or(crate::install::types::InstallError::HomeDirNotFound)?
                        .join(format!(".{}/skills/workflow", id));

                    let skills_count = self.copy_skills_to(&target_dir)?;
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

    /// 复制技能到目标目录（递归复制）
    pub fn copy_skills_to(&self, target_dir: &Path) -> Result<usize> {
        // 创建目标目录
        fs::create_dir_all(target_dir)?;

        // 递归复制技能文件和目录
        fn copy_recursive(source: &Path, target: &Path) -> std::io::Result<usize> {
            let mut count = 0;

            if source.is_file() {
                fs::copy(source, target)?;
                return Ok(1);
            }

            if source.is_dir() {
                if let Ok(entries) = fs::read_dir(source) {
                    for entry in entries.flatten() {
                        let source_path = entry.path();
                        let target_path = target.join(entry.file_name());

                        if source_path.is_dir() {
                            fs::create_dir_all(&target_path)?;
                            count += copy_recursive(&source_path, &target_path)?;
                        } else {
                            fs::copy(&source_path, &target_path)?;
                            count += 1;
                        }
                    }
                }
            }

            Ok(count)
        }

        let total_count = copy_recursive(&self.source_dir, target_dir)?;
        Ok(total_count)
    }

    /// 获取目标目录列表
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

    /// 计算目录中的技能文件数量（递归统计）
    #[allow(dead_code)]
    pub fn count_skills_in_dir(&self, dir: &Path) -> Result<usize> {
        fn count_files_recursive(path: &Path) -> std::io::Result<usize> {
            let mut total = 0;
            if path.is_file() {
                return Ok(1);
            }

            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    total += count_files_recursive(&entry_path)?;
                }
            }

            Ok(total)
        }

        count_files_recursive(dir).map_err(From::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_skills(temp_dir: &TempDir) {
        let skills_dir = temp_dir.path().join(".loom/skills/auto");
        fs::create_dir_all(&skills_dir).unwrap();

        fs::write(skills_dir.join("test.md"), "# Test").unwrap();
        fs::write(skills_dir.join("workflow.md"), "# Workflow").unwrap();
        fs::write(skills_dir.join("readme.txt"), "Readme content").unwrap();
    }

    #[test]
    fn test_skill_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_skills(&temp_dir);

        // 临时改变工作目录
        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let result = SkillManager::new();

        // 恢复工作目录
        std::env::set_current_dir(original_dir).unwrap();

        assert!(result.is_ok());
    }

    #[test]
    fn test_skill_manager_source_not_found() {
        // 在没有技能源目录的情况下
        let result = SkillManager::new();
        assert!(result.is_err());
        match result {
            Err(crate::install::types::InstallError::SkillSourceNotFound(_)) => {}
            _ => panic!("Expected SkillSourceNotFound error"),
        }
    }

    #[test]
    fn test_copy_skills_to_directory() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_skills(&temp_dir);

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let skill_manager = SkillManager::new().unwrap();
        let target_dir = temp_dir.path().join("target_skills");

        let count = skill_manager.copy_skills_to(&target_dir).unwrap();
        assert_eq!(count, 3);

        assert!(target_dir.join("test.md").exists());
        assert!(target_dir.join("workflow.md").exists());
        assert!(target_dir.join("readme.txt").exists());

        std::env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    fn test_install_for_codex() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_skills(&temp_dir);

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let skill_manager = SkillManager::new().unwrap();
        let results = skill_manager
            .install_for_agents(&[AgentType::Codex])
            .unwrap();

        std::env::set_current_dir(original_dir).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].skills_count, 3);
        assert!(results[0].agent_type.contains(&AgentType::Codex));
    }

    #[test]
    fn test_install_for_claude() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_skills(&temp_dir);

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let skill_manager = SkillManager::new().unwrap();
        let results = skill_manager
            .install_for_agents(&[AgentType::Claude])
            .unwrap();

        std::env::set_current_dir(original_dir).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].skills_count, 3);
        assert!(results[0].agent_type.contains(&AgentType::Claude));
    }

    #[test]
    fn test_install_for_multiple_agents() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_skills(&temp_dir);

        let original_dir = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp_dir.path()).unwrap();

        let skill_manager = SkillManager::new().unwrap();
        let results = skill_manager
            .install_for_agents(&[AgentType::Codex, AgentType::Claude])
            .unwrap();

        std::env::set_current_dir(original_dir).unwrap();

        assert_eq!(results.len(), 2);
    }
}
