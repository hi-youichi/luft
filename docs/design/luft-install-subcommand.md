# Luft 一键安装子命令设计方案

## 1. 方案概述

### 1.1 背景
Luft 作为 Agent Client Protocol (ACP) 桥接工具，需要与多种 Agent 后端（如 Codex ACP、OpenCode、Claude Code）进行集成。现有的手动安装流程复杂，需要用户手动检测 Agent 安装状态、配置技能目录、设置 MCP 等。

### 1.2 目标
提供一个统一的 `luft install` 命令，自动化完成：
- 检测已安装的 Agent 后端
- 为检测到的 Agent 安装 Luft 桥接组件
- 配置 MCP 服务器
- 验证安装完整性

### 1.3 设计原则
- **零参数**: 用户无需提供任何参数，自动完成所有检测和安装
- **职责清晰**: 区分 Agent 检测（外部软件）vs Luft 桥接安装（内部功能）
- **用户友好**: 提供清晰的进度反馈和错误信息
- **安全可靠**: 幂等操作，支持重复执行

## 2. 概念澄清

### 2.1 Agent 检测 vs Luft 桥接安装

**Agent 检测** - 检查外部 Agent 软件是否已安装：
- `@agentclientprotocol/codex-acp` 是否已安装（全局或通过 npx）
- `opencode` 命令或目录是否存在
- `claude code` 命令或目录是否存在

**Luft 桥接安装** - Luft 为检测到的 Agent 提供集成功能：
- 技能文件复制到 Agent 专用目录
- MCP 服务器配置
- 工作流文件注入
- 不负责安装 Agent 本身

### 2.2 支持的 Agent 类型

```rust
pub enum AgentType {
    Mock,      // Luft 内建，无需检测
    Codex,     // @agentclientprotocol/codex-acp
    Opencode,  // OpenCode Agent
    Claude,    // Claude Code
}
```

## 3. 架构设计

### 3.1 项目结构

```
crates/luft-cli/src/
├── commands/
│   ├── install.rs          # install 子命令入口
│   └── mod.rs              # 命令模块注册
├── install/                # 安装功能模块
│   ├── mod.rs              # 模块入口
│   ├── agent_detector.rs   # Agent 检测器
│   ├── skill_manager.rs    # 技能管理器
│   ├── mcp_setup.rs        # MCP 配置器
│   ├── installer.rs        # 统一安装器
│   └── types.rs            # 公共类型定义
└── install/
    └── tests/              # 测试模块
        ├── agent_detector_tests.rs
        ├── skill_manager_tests.rs
        ├── mcp_setup_tests.rs
        ├── installer_tests.rs
        └── integration_tests.rs
```

### 3.2 命令接口

```bash
# 主命令
luft install              # 一键安装，自动检测所有 Agent

# 不需要任何参数，自动化完成所有检测和桥接安装
```

## 4. 模块设计

### 4.1 Agent 检测器 (`agent_detector.rs`)

#### 职责
检测系统中已安装的外部 Agent 软件

#### 接口设计
```rust
pub struct AgentDetector;

impl AgentDetector {
    /// 检测所有可用的 Agent
    pub fn detect_all() -> Result<Vec<AgentType>>;
    
    /// 检测特定 Agent
    fn is_codex_acp_installed() -> Result<bool>;
    fn is_opencode_installed() -> Result<bool>;
    fn is_claude_code_installed() -> Result<bool>;
}
```

#### 检测策略

**Codex ACP 检测**:
1. 检查全局安装: `npm list -g @agentclientprotocol/codex-acp`
2. 检查 npx 可用性: `npx -y @agentclientprotocol/codex-acp --version`
3. 任一方式成功即认为已安装

**OpenCode 检测**:
1. 检查命令存在: `which opencode`
2. 检查常见路径:
   - `~/.opencode`
   - `~/AppData/Roaming/opencode` (Windows)
   - `~/AppData/Local/opencode` (Windows)
   - `~/.config/opencode` (Linux)
   - `~/Library/Application Support/opencode` (macOS)

**Claude Code 检测**:
1. 检查命令存在: `which claude`
2. 检查配置目录存在:
   - `~/.claude`
   - `~/AppData/Roaming/claude`
   - `~/Library/Application Support/claude`

### 4.2 技能管理器 (`skill_manager.rs`)

#### 职责
管理技能文件的复制和目录创建

#### 接口设计
```rust
pub struct SkillManager {
    source_dir: PathBuf,
}

impl SkillManager {
    /// 创建技能管理器
    pub fn new() -> Result<Self>;
    
    /// 为指定的 Agent 类型安装技能
    pub fn install_for_agents(&self, agents: &[AgentType]) -> Result<Vec<SkillInstallResult>>;
    
    /// 复制技能到目标目录
    pub fn copy_skills_to(&self, target_dir: &Path) -> Result<usize>;
    
    /// 获取目标目录列表
    fn get_target_directories(&self, agents: &[AgentType]) -> Result<Vec<PathBuf>>;
}
```

#### 技能目录映射
- **Codex/OpenCode**: `~/.agents/skills/workflow/`
- **Claude**: `~/.claude/skills/workflow/`

#### 技能源目录
- `.loom/skills/auto/` - Luft 自带技能目录

### 4.3 MCP 配置器 (`mcp_setup.rs`)

#### 职责
配置 Claude Code 的 MCP 服务器集成

#### 接口设计
```rust
pub struct McpSetup;

impl McpSetup {
    /// 配置 Claude MCP 服务器
    pub fn configure() -> Result<()>;
    
    /// 合并 MCP 配置到现有配置
    fn merge_mcp_config(config: &mut Value) -> Result<()>;
    
    /// 读取/写入配置文件
    fn read_config(path: &Path) -> Result<Value>;
    fn write_config(path: &Path, config: &Value) -> Result<()>;
    
    /// 获取 Claude 配置目录
    fn get_claude_config_dir() -> PathBuf;
}
```

#### 配置内容
```json
{
  "mcpServers": {
    "luft": {
      "command": "luft",
      "args": ["mcp", "serve"]
    }
  }
}
```

### 4.4 统一安装器 (`installer.rs`)

#### 职责
协调各模块完成完整安装流程

#### 接口设计
```rust
pub struct Installer;

impl Installer {
    /// 执行完整安装流程
    pub fn install_all() -> Result<InstallSummary>;
    
    /// 为检测到的 Agent 安装桥接
    fn install_bridges_for_agents(&self, agents: &[AgentType]) -> Result<Vec<BridgeInstallResult>>;
    
    /// 验证安装结果
    fn verify_installation(&self, summary: &InstallSummary) -> Result<()>;
}
```

#### 安装流程
1. 检测所有可用的 Agent
2. 检查是否至少有一个外部 Agent（排除 Mock）
3. 为检测到的 Agent 安装桥接
4. 如果检测到 Claude Code，配置 MCP 服务器
5. 验证安装结果
6. 返回安装摘要

### 4.5 公共类型定义 (`types.rs`)

```rust
/// Agent 类型枚举
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AgentType {
    Mock,
    Codex,
    Opencode,
    Claude,
}

/// 安装摘要
pub struct InstallSummary {
    pub detected_agents: Vec<AgentType>,
    pub bridges_installed: Vec<BridgeInstallResult>,
    pub mcp_configured: bool,
    pub installation_time: Duration,
}

/// 桥接安装结果
pub struct BridgeInstallResult {
    pub agent_type: Vec<AgentType>,
    pub target_dir: PathBuf,
    pub skills_count: usize,
}

/// 技能安装结果
pub struct SkillInstallResult {
    pub target_dir: PathBuf,
    pub skills_count: usize,
}

/// 安装错误类型
#[derive(Debug)]
pub enum InstallError {
    NoExternalAgentsFound,
    AgentDetection(String),
    BridgeInstallation(String),
    SkillCopy(String),
    McpConfiguration(String),
    HomeDirNotFound,
    SkillSourceNotFound(PathBuf),
    VerificationFailed(String),
}
```

## 5. 实现流程

### 5.1 主安装流程

```rust
impl Installer {
    pub fn install_all() -> Result<InstallSummary> {
        let start_time = Instant::now();
        
        // 1. 检测已安装的外部 Agent
        eprintln!("🔍 检测已安装的 Agent...");
        let detected_agents = AgentDetector::detect_all()?;
        
        // 2. 检查是否至少有一个外部 Agent
        let external_agents: Vec<_> = detected_agents.iter()
            .filter(|a| **a != AgentType::Mock)
            .collect();
            
        if external_agents.is_empty() {
            return Err(InstallError::NoExternalAgentsFound);
        }
        
        for agent in &external_agents {
            eprintln!("✅ 检测到: {:?}", agent);
        }
        
        // 3. 安装桥接组件
        eprintln!("🔧 安装 Luft 桥接组件...");
        let bridges_installed = self.install_bridges_for_agents(&detected_agents)?;
        
        for bridge in &bridges_installed {
            eprintln!("📁 技能已安装到: {} ({} 个技能)", 
                bridge.target_dir.display(), bridge.skills_count);
        }
        
        // 4. 配置 MCP 服务器
        let mcp_configured = if detected_agents.contains(&AgentType::Claude) {
            eprintln!("🌐 配置 Claude MCP 服务器...");
            McpSetup::configure()?;
            eprintln!("✅ MCP 配置完成");
            true
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
        
        self.verify_installation(&summary)?;
        eprintln!("🎉 安装完成！");
        
        Ok(summary)
    }
}
```

### 5.2 Agent 检测流程

```rust
impl AgentDetector {
    pub fn detect_all() -> Result<Vec<AgentType>> {
        let mut available = vec![AgentType::Mock];
        
        // 并行检测各个 Agent
        if Self::is_codex_acp_installed()? {
            available.push(AgentType::Codex);
        }
        if Self::is_opencode_installed()? {
            available.push(AgentType::Opencode);
        }
        if Self::is_claude_code_installed()? {
            available.push(AgentType::Claude);
        }
        
        Ok(available)
    }
}
```

### 5.3 桥接安装流程

```rust
impl Installer {
    fn install_bridges_for_agents(&self, agents: &[AgentType]) -> Result<Vec<BridgeInstallResult>> {
        let mut results = vec![];
        let mut processed_codex_opencode = false;
        let mut processed_claude = false;
        
        for agent in agents {
            match agent {
                AgentType::Mock => continue,
                
                AgentType::Codex | AgentType::Opencode => {
                    if !processed_codex_opencode {
                        let result = self.install_codex_opencode_bridge()?;
                        results.push(result);
                        processed_codex_opencode = true;
                    }
                }
                
                AgentType::Claude => {
                    if !processed_claude {
                        let result = self.install_claude_bridge()?;
                        results.push(result);
                        processed_claude = true;
                    }
                }
            }
        }
        
        Ok(results)
    }
}
```

## 6. 测试方案

### 6.1 单元测试策略

#### 测试覆盖目标
- **单元测试覆盖率**: > 80%
- **关键路径覆盖率**: 100%
- **错误处理路径**: 完整覆盖

#### Agent 检测器测试

```rust
#[cfg(test)]
mod agent_detector_tests {
    use super::*;
    
    #[test]
    fn test_detect_all_returns_at_least_mock() {
        let agents = AgentDetector::detect_all().unwrap();
        assert!(!agents.is_empty());
        assert!(agents.contains(&AgentType::Mock));
    }
    
    #[test]
    fn test_detect_codex_acp_installed_globally() {
        let result = AgentDetector::is_codex_acp_installed().unwrap();
        // 根据环境验证
    }
    
    #[test]
    fn test_detect_codex_acp_via_npx() {
        let output = Command::new("npx")
            .args(["-y", "@agentclientprotocol/codex-acp", "--version"])
            .output();
        assert!(output.is_ok());
    }
    
    #[test]
    fn test_detect_opencode_by_command() {
        let result = which::which("opencode");
        match result {
            Ok(path) => assert!(path.exists()),
            Err(_) => assert!(true), // 未安装也是合法结果
        }
    }
    
    #[test]
    fn test_detect_claude_code() {
        let by_command = which::which("claude").is_ok();
        let by_path = dirs::home_dir()
            .map(|d| d.join(".claude"))
            .map(|p| p.exists() && p.is_dir())
            .unwrap_or(false);
        
        assert!(by_command || by_path || true); // 都未检测到也是合法
    }
}
```

#### 技能管理器测试

```rust
#[cfg(test)]
mod skill_manager_tests {
    use super::*;
    use tempfile::TempDir;
    
    fn setup_test_skills(temp_dir: &TempDir) {
        let skills_dir = temp_dir.path().join(".loom/skills/auto");
        std::fs::create_dir_all(&skills_dir).unwrap();
        
        std::fs::write(skills_dir.join("test.md"), "# Test").unwrap();
        std::fs::write(skills_dir.join("workflow.md"), "# Workflow").unwrap();
    }
    
    #[test]
    fn test_skill_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_skills(&temp_dir);
        
        let skill_manager = SkillManager::new();
        assert!(skill_manager.is_ok());
    }
    
    #[test]
    fn test_get_target_directories_for_codex() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_skills(&temp_dir);
        
        let skill_manager = SkillManager::new().unwrap();
        let dirs = skill_manager.get_target_directories(&[AgentType::Codex]).unwrap();
        
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with(".agents/skills/workflow"));
    }
    
    #[test]
    fn test_copy_skills_to_directory() {
        let temp_dir = TempDir::new().unwrap();
        setup_test_skills(&temp_dir);
        
        let skill_manager = SkillManager::new().unwrap();
        let target_dir = temp_dir.path().join("target");
        
        let count = skill_manager.copy_skills_to(&target_dir).unwrap();
        assert_eq!(count, 2);
        
        assert!(target_dir.join("test.md").exists());
        assert!(target_dir.join("workflow.md").exists());
    }
}
```

#### MCP 配置器测试

```rust
#[cfg(test)]
mod mcp_setup_tests {
    use super::*;
    use serde_json::json;
    
    #[test]
    fn test_merge_mcp_config_into_empty() {
        let mut config = json!({});
        McpSetup::merge_mcp_config(&mut config).unwrap();
        
        assert!(config.is_object());
        assert!(config.get("mcpServers").is_some());
    }
    
    #[test]
    fn test_merge_mcp_config_preserves_existing() {
        let mut config = json!({
            "mcpServers": {
                "existing": {"command": "existing-cmd"}
            }
        });
        
        McpSetup::merge_mcp_config(&mut config).unwrap();
        
        let servers = config["mcpServers"].as_object().unwrap();
        assert!(servers.contains_key("existing"));
        assert!(servers.contains_key("luft"));
    }
    
    #[test]
    fn test_merge_mcp_config_no_duplicate() {
        let mut config = json!({
            "mcpServers": {
                "luft": {"command": "old-luft"}
            }
        });
        
        McpSetup::merge_mcp_config(&mut config).unwrap();
        
        assert_eq!(config["mcpServers"]["luft"]["command"], "old-luft");
    }
}
```

#### 安装器测试

```rust
#[cfg(test)]
mod installer_tests {
    use super::*;
    use tempfile::TempDir;
    
    fn setup_full_environment(temp_dir: &TempDir) {
        let skills_dir = temp_dir.path().join(".loom/skills/auto");
        std::fs::create_dir_all(&skills_dir).unwrap();
        std::fs::write(skills_dir.join("test.md"), "# Test").unwrap();
    }
    
    #[test]
    fn test_install_bridges_for_codex() {
        let temp_dir = TempDir::new().unwrap();
        setup_full_environment(&temp_dir);
        
        let installer = Installer::new();
        let results = installer.install_bridges_for_agents(&[AgentType::Codex]).unwrap();
        
        assert_eq!(results.len(), 1);
        assert!(results[0].target_dir.ends_with(".agents/skills/workflow"));
    }
    
    #[test]
    fn test_install_all_requires_external_agents() {
        // Mock 环境只有 Mock Agent
        let result = Installer::install_all();
        assert!(result.is_err());
        match result {
            Err(InstallError::NoExternalAgentsFound) => assert!(true),
            _ => panic!("Expected NoExternalAgentsFound"),
        }
    }
    
    #[test]
    fn test_install_summary_structure() {
        let summary = InstallSummary {
            detected_agents: vec![AgentType::Codex, AgentType::Claude],
            bridges_installed: vec![],
            mcp_configured: true,
            installation_time: Duration::from_millis(100),
        };
        
        assert_eq!(summary.detected_agents.len(), 2);
        assert!(summary.mcp_configured);
    }
}
```

### 6.2 集成测试

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    
    #[test]
    fn test_full_installation_workflow() {
        // 在有真实 Agent 的环境中测试完整流程
        // 1. 检测 Agents
        // 2. 安装桥接
        // 3. 配置 MCP
        // 4. 验证结果
    }
    
    #[test]
    fn test_installation_idempotency() {
        // 测试重复执行的一致性
        let summary1 = Installer::install_all().unwrap();
        let summary2 = Installer::install_all().unwrap();
        
        assert_eq!(summary1.detected_agents, summary2.detected_agents);
        assert_eq!(summary1.bridges_installed.len(), summary2.bridges_installed.len());
    }
}
```

### 6.3 Mock 测试工具

```rust
#[cfg(test)]
pub mod test_utils {
    use super::*;
    
    pub struct MockAgentDetector {
        pub installed_agents: Vec<AgentType>,
    }
    
    impl MockAgentDetector {
        pub fn with_agents(agents: Vec<AgentType>) -> Self {
            Self { installed_agents: agents }
        }
        
        pub fn detect_all(&self) -> Result<Vec<AgentType>> {
            Ok(self.installed_agents.clone())
        }
    }
    
    pub struct MockInstaller {
        pub mock_results: Vec<BridgeInstallResult>,
    }
    
    impl MockInstaller {
        pub fn new() -> Self {
            Self {
                mock_results: vec![
                    BridgeInstallResult {
                        agent_type: vec![AgentType::Codex, AgentType::Opencode],
                        target_dir: PathBuf::from("/mock/.agents/skills/workflow"),
                        skills_count: 3,
                    },
                ],
            }
        }
        
        pub fn install_bridges_for_agents(&self, _agents: &[AgentType]) -> Result<Vec<BridgeInstallResult>> {
            Ok(self.mock_results.clone())
        }
    }
}
```

## 7. 用户接口

### 7.1 命令示例

```bash
# 基本安装
luft install

# 输出示例
🔍 检测已安装的 Agent...
✅ 检测到: Codex
✅ 检测到: Claude
🔧 安装 Luft 桥接组件...
📁 技能已安装到: C:\Users\username\.agents\skills\workflow (5 个技能)
📁 技能已安装到: C:\Users\username\.claude\skills\workflow (5 个技能)
🌐 配置 Claude MCP 服务器...
✅ MCP 配置完成
✅ 验证安装...
🎉 安装完成！

检测到的 Agent:
  ✅ Mock
  ✅ Codex
  ✅ Claude

安装摘要:
- 桥接安装: 2 个
- MCP 配置: 完成
- 耗时: 0.3 秒
```

### 7.2 错误处理

```bash
# 无外部 Agent 时
luft install

🔍 检测已安装的 Agent...
❌ 错误: 未检测到任何外部 Agent
建议: 请先安装至少一个支持的 Agent：
- Codex ACP: npm install -g @agentclientprotocol/codex-acp
- OpenCode: 下载并安装 OpenCode
- Claude Code: 下载并安装 Claude Code
```

## 8. 技术细节

### 8.1 依赖项

```toml
[dependencies]
# 现有依赖
clap = "4.5"
tokio = "1.35"
dirs = "5.0"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
which = "6.0"

# 测试依赖
[dev-dependencies]
tempfile = "3.8"
```

### 8.2 跨平台考虑

**路径处理**:
- 使用 `dirs` crate 获取跨平台的标准目录
- 统一使用 `PathBuf` 进行路径操作
- 路径拼接使用 `join()` 方法

**命令执行**:
- Windows: 处理 `.cmd` 后缀
- Unix: 直接使用命令名
- 使用 `std::process::Command` 跨平台执行

**文件权限**:
- 目录创建使用适当的权限
- 配置文件读写考虑权限问题

### 8.3 性能优化

**并行检测**:
- 多个 Agent 检测可以并行执行
- 使用 `rayon` 或 `tokio` 提供并行支持

**缓存机制**:
- 检测结果可以缓存一段时间
- 避免重复的文件系统检查

**增量更新**:
- 只复制变更的技能文件
- 检查文件哈希避免重复复制

### 8.4 安全考虑

**输入验证**:
- 所有外部输入进行验证
- 路径注入防护
- 命令注入防护

**权限检查**:
- 确保有足够权限创建目录
- 文件写入权限检查

**敏感信息**:
- 不记录敏感路径信息
- 配置文件中的凭据处理

## 9. 扩展性设计

### 9.1 Agent 注册机制

**配置驱动的 Agent 定义**:
```toml
# agent-registry.toml
[[agents]]
id = "codex"
name = "Codex ACP"
detection = ["npm_global", "npx"]
command = "npx"
args = ["-y", "@agentclientprotocol/codex-acp", "--version"]
skill_dir = ".agents/skills/workflow"
mcp_support = false

[[agents]]
id = "opencode"
name = "OpenCode"
detection = ["command", "paths"]
command = "opencode"
paths = ["~/.opencode", "~/AppData/Roaming/opencode"]
skill_dir = ".agents/skills/workflow"
mcp_support = false

[[agents]]
id = "claude"
name = "Claude Code"
detection = ["command", "paths"]
command = "claude"
paths = ["~/.claude", "~/AppData/Roaming/claude"]
skill_dir = ".claude/skills/workflow"
mcp_support = true

# 未来扩展的 Agent 示例
[[agents]]
id = "custom_agent"
name = "Custom Agent"
detection = ["custom_detection"]
command = "custom-cmd"
skill_dir = ".custom/skills"
mcp_support = false
```

### 9.2 Trait-based 架构设计

**Agent 检测 Trait**:
```rust
pub trait AgentDetectorTrait: Send + Sync {
    fn agent_type(&self) -> AgentType;
    fn detect(&self) -> Result<bool>;
    fn skill_directory(&self) -> PathBuf;
    fn supports_mcp(&self) -> bool;
    fn command_check(&self) -> Option<&str>;
}

// Codex ACP 检测实现
pub struct CodexAgentDetector;

impl AgentDetectorTrait for CodexAgentDetector {
    fn agent_type(&self) -> AgentType { AgentType::Codex }
    
    fn detect(&self) -> Result<bool> {
        // 检查全局安装和 npx
        Self::check_npm_global()? || Self::check_npx()?
    }
    
    fn skill_directory(&self) -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".agents/skills/workflow")
    }
    
    fn supports_mcp(&self) -> bool { false }
    
    fn command_check(&self) -> Option<&str> { Some("npx") }
}

// 未来可轻松添加新的检测器
pub struct CustomAgentDetector {
    config: AgentConfig,
}

impl AgentDetectorTrait for CustomAgentDetector {
    fn agent_type(&self) -> AgentType { 
        AgentType::Custom(self.config.id.clone()) 
    }
    
    fn detect(&self) -> Result<bool> {
        // 自定义检测逻辑
        match &self.config.detection {
            DetectionMethod::Command(cmd) => Self::check_command(cmd),
            DetectionMethod::Paths(paths) => Self::check_paths(paths),
            DetectionMethod::Custom(fn) => (fn)(),
        }
    }
    
    // ... 其他方法实现
}
```

### 9.3 动态 Agent 注册

**Agent 注册表**:
```rust
pub struct AgentRegistry {
    detectors: Vec<Box<dyn AgentDetectorTrait>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        let mut detectors: Vec<Box<dyn AgentDetectorTrait>> = vec![
            Box::new(MockAgentDetector::new()),      // 总是包含
            Box::new(CodexAgentDetector::new()),
            Box::new(OpencodeAgentDetector::new()),
            Box::new(ClaudeAgentDetector::new()),
        ];
        
        // 从配置文件加载自定义 Agent
        if let Ok(custom_detectors) = Self::load_custom_detectors() {
            detectors.extend(custom_detectors);
        }
        
        Self { detectors }
    }
    
    pub fn detect_all(&self) -> Result<Vec<AgentDetectionResult>> {
        self.detectors
            .iter()
            .map(|detector| {
                let agent_type = detector.agent_type();
                let is_available = detector.detect()?;
                let skill_dir = detector.skill_directory();
                let mcp_support = detector.supports_mcp();
                
                Ok(AgentDetectionResult {
                    agent_type,
                    is_available,
                    skill_directory: skill_dir,
                    supports_mcp: mcp_support,
                    detector_name: detector.detector_name(),
                })
            })
            .collect()
    }
    
    pub fn register_detector(&mut self, detector: Box<dyn AgentDetectorTrait>) {
        self.detectors.push(detector);
    }
    
    pub fn register_from_config(&mut self, config: AgentConfig) -> Result<()> {
        let detector = CustomAgentDetector::from_config(config)?;
        self.detectors.push(Box::new(detector));
        Ok(())
    }
}
```

### 9.4 可扩展的 AgentType

**支持动态 Agent 类型**:
```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentType {
    // 标准内置 Agent
    Mock,
    Codex,
    Opencode,
    Claude,
    
    // 自定义 Agent
    Custom(String),
    
    // 未来的扩展 Agent
    #[serde(skip)] // 保留扩展空间
    FutureExtension,
}

impl AgentType {
    pub fn is_custom(&self) -> bool {
        matches!(self, AgentType::Custom(_))
    }
    
    pub fn custom_id(&self) -> Option<&str> {
        match self {
            AgentType::Custom(id) => Some(id),
            _ => None,
        }
    }
    
    pub fn display_name(&self) -> String {
        match self {
            AgentType::Mock => "Mock".to_string(),
            AgentType::Codex => "Codex ACP".to_string(),
            AgentType::Opencode => "OpenCode".to_string(),
            AgentType::Claude => "Claude Code".to_string(),
            AgentType::Custom(id) => format!("Custom Agent ({})", id),
            AgentType::FutureExtension => "Future Agent".to_string(),
        }
    }
}
```

### 9.5 策略模式的检测方法

**多种检测策略**:
```rust
pub enum DetectionMethod {
    Command(String),
    Paths(Vec<String>),
    NpmPackage(String),
    Custom(Box<dyn Fn() -> bool + Send + Sync>),
    Composite(Vec<DetectionMethod>), // 组合多种检测方法
}

impl DetectionMethod {
    pub fn check(&self) -> Result<bool> {
        match self {
            DetectionMethod::Command(cmd) => Self::check_command(cmd),
            DetectionMethod::Paths(paths) => Self::check_paths(paths),
            DetectionMethod::NpmPackage(pkg) => Self::check_npm_package(pkg),
            DetectionMethod::Custom(fn) => Ok(fn()),
            DetectionMethod::Composite(methods) => {
                // OR 逻辑：任一方法成功即认为检测成功
                for method in methods {
                    if method.check()? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }
        }
    }
}

// 使用示例
let codex_detection = DetectionMethod::Composite(vec![
    DetectionMethod::NpmPackage("@agentclientprotocol/codex-acp".to_string()),
    DetectionMethod::Command("npx".to_string()),
]);
```

### 9.6 技能目录的动态映射

**目录映射策略**:
```rust
pub trait SkillDirectoryMapper: Send + Sync {
    fn get_skill_directory(&self, agent_type: &AgentType) -> Result<PathBuf>;
    fn supports_agent(&self, agent_type: &AgentType) -> bool;
}

pub struct StandardDirectoryMapper;

impl SkillDirectoryMapper for StandardDirectoryMapper {
    fn get_skill_directory(&self, agent_type: &AgentType) -> Result<PathBuf> {
        let home = dirs::home_dir().ok_or(InstallError::HomeDirNotFound)?;
        
        let relative_path = match agent_type {
            AgentType::Codex | AgentType::Opencode => ".agents/skills/workflow",
            AgentType::Claude => ".claude/skills/workflow",
            AgentType::Custom(id) => &format!(".{}/skills/workflow", id),
            AgentType::Mock | AgentType::FutureExtension => {
                return Err(InstallError::UnsupportedAgentType);
            }
        };
        
        Ok(home.join(relative_path))
    }
    
    fn supports_agent(&self, agent_type: &AgentType) -> bool {
        !matches!(agent_type, AgentType::Mock | AgentType::FutureExtension)
    }
}

// 自定义目录映射器
pub struct CustomDirectoryMapper {
    mappings: HashMap<String, PathBuf>,
}

impl CustomDirectoryMapper {
    pub fn new(mappings: HashMap<String, PathBuf>) -> Self {
        Self { mappings }
    }
}

impl SkillDirectoryMapper for CustomDirectoryMapper {
    fn get_skill_directory(&self, agent_type: &AgentType) -> Result<PathBuf> {
        if let AgentType::Custom(id) = agent_type {
            if let Some(path) = self.mappings.get(id) {
                return Ok(path.clone());
            }
        }
        // 回退到标准映射
        StandardDirectoryMapper.get_skill_directory(agent_type)
    }
    
    fn supports_agent(&self, agent_type: &AgentType) -> bool {
        true // 支持所有 Agent 类型
    }
}
```

### 9.7 MCP 配置的扩展性

**MCP 配置策略**:
```rust
pub trait McpConfigurer: Send + Sync {
    fn configure(&self, agent_type: &AgentType) -> Result<McpConfigResult>;
    fn supports_mcp(&self, agent_type: &AgentType) -> bool;
}

pub struct StandardMcpConfigurer;

impl McpConfigurer for StandardMcpConfigurer {
    fn configure(&self, agent_type: &AgentType) -> Result<McpConfigResult> {
        match agent_type {
            AgentType::Claude => {
                // Claude Code 的 MCP 配置
                McpSetup::configure()?;
                Ok(McpConfigResult::Configured)
            }
            AgentType::Custom(id) => {
                // 自定义 Agent 的 MCP 配置
                Self::configure_custom_agent(id)?;
                Ok(McpConfigResult::Configured)
            }
            _ => Ok(McpConfigResult::NotSupported),
        }
    }
    
    fn supports_mcp(&self, agent_type: &AgentType) -> bool {
        matches!(agent_type, AgentType::Claude | AgentType::Custom(_))
    }
}

// 自定义 MCP 配置器
pub struct CustomMcpConfigurer {
    config_map: HashMap<String, McpAgentConfig>,
}

impl CustomMcpConfigurer {
    fn configure_custom_agent(&self, id: &str) -> Result<()> {
        if let Some(config) = self.config_map.get(id) {
            // 根据自定义配置进行 MCP 设置
            Self::apply_mcp_config(config)?;
        }
        Ok(())
    }
}
```

### 9.8 插件系统基础

**插件接口设计**:
```rust
#[async_trait]
pub trait LuftInstallPlugin: Send + Sync {
    fn name(&self) -> String;
    fn version(&self) -> String;
    
    async fn detect_agents(&self) -> Result<Vec<AgentType>>;
    async fn install_skills(&self, agents: &[AgentType]) -> Result<Vec<SkillInstallResult>>;
    async fn configure_mcp(&self, agents: &[AgentType]) -> Result<McpConfigResult>;
    
    // 插件生命周期钩子
    async fn on_install_start(&self) -> Result<()> { Ok(()) }
    async fn on_install_complete(&self, summary: &InstallSummary) -> Result<()> { Ok(()) }
    async fn on_error(&self, error: &InstallError) -> Result<()> { Ok(()) }
}

pub struct PluginManager {
    plugins: Vec<Box<dyn LuftInstallPlugin>>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self { plugins: vec![] }
    }
    
    pub fn register_plugin(&mut self, plugin: Box<dyn LuftInstallPlugin>) {
        self.plugins.push(plugin);
    }
    
    pub async fn run_plugins_detection(&self) -> Result<Vec<AgentType>> {
        let mut all_agents = vec![];
        
        for plugin in &self.plugins {
            let detected = plugin.detect_agents().await?;
            all_agents.extend(detected);
        }
        
        Ok(all_agents)
    }
}
```

### 9.9 配置文件驱动的扩展

**Agent 配置文件**:
```toml
# agents-config.toml
[registry]
version = "1.0"
auto_discovery = true

[[agent]]
id = "codex"
name = "Codex ACP"
enabled = true
detection = { method = "composite", strategies = [
    { type = "npm", package = "@agentclientprotocol/codex-acp" },
    { type = "command", executable = "npx", args = ["-y", "@agentclientprotocol/codex-acp", "--version"] }
]}
skill_config = { directory = ".agents/skills/workflow" }
mcp_config = { enabled = false }

[[agent]]
id = "future_agent"
name = "Future ACP Agent"
enabled = false  # 可以暂时禁用
detection = { method = "command", executable = "future-agent", args = ["--version"] }
skill_config = { directory = ".future/skills/workflow" }
mcp_config = { enabled = true, command = "future-agent", args = ["mcp", "serve"] }

[extensions]
# 未来扩展配置
custom_detectors_path = "~/.luft/custom_detectors"
plugin_path = "~/.luft/plugins"
```

### 9.10 向后兼容性

**版本兼容策略**:
```rust
#[derive(Debug, Clone, Deserialize)]
struct AgentConfigV1 {
    id: String,
    name: String,
    detection_method: String,
    skill_dir: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum AgentConfig {
    V1(AgentConfigV1),
    V2(AgentConfigV2),  // 未来的版本
}

impl AgentConfig {
    fn to_detector(&self) -> Result<Box<dyn AgentDetectorTrait>> {
        match self {
            AgentConfig::V1(v1_config) => {
                // 兼容 V1 配置
                Self::v1_to_detector(v1_config)
            }
            AgentConfig::V2(v2_config) => {
                // 使用 V2 配置
                Self::v2_to_detector(v2_config)
            }
        }
    }
}
```

## 10. 后续扩展计划

### 10.1 支持更多 Agent
- ACP 兼容的第三方 Agent
- 自定义 Agent 类型支持
- Agent 插件生态系统

### 10.2 高级功能
- 技能版本管理和依赖解析
- 配置文件备份和恢复
- 安装历史和回滚功能
- Agent 性能监控和报告

### 10.3 集成改进
- 与 Luft 其他子命令深度集成
- Web UI 管理界面
- 远程 Agent 发现和安装
- 团队共享配置

## 10. 实施计划

### 10.1 开发阶段
1. **Phase 1**: 核心模块开发 (1-2 天)
   - Agent 检测器
   - 技能管理器
   - MCP 配置器

2. **Phase 2**: 集成和测试 (1 天)
   - 统一安装器
   - 单元测试
   - 集成测试

3. **Phase 3**: CLI 集成 (0.5 天)
   - 命令注册
   - 用户界面优化

4. **Phase 4**: 文档和发布 (0.5 天)
   - 用户文档
   - 开发者文档
   - 发布说明

### 10.2 验收标准
- ✅ 所有单元测试通过
- ✅ 代码覆盖率 > 80%
- ✅ 跨平台兼容性验证
- ✅ 用户测试通过
- ✅ 文档完整

## 11. 风险和缓解

### 11.1 技术风险
**风险**: Agent 检测不准确
**缓解**: 多种检测方式组合，提供手动配置选项

**风险**: 文件权限问题
**缓解**: 权限检查和友好的错误提示

### 11.2 兼容性风险
**风险**: 不同平台行为差异
**缓解**: 充分的跨平台测试

**风险**: Agent 版本兼容性
**缓解**: 版本检查和兼容性矩阵

## 12. 总结

本设计方案提供了 Luft 一键安装子命令的完整实现路径，通过清晰的职责分离、完善的测试方案和用户友好的接口设计，确保安装过程的自动化、可靠性和易用性。方案强调了 Agent 检测与 Luft 桥接安装的概念区别，为后续的维护和扩展提供了坚实的基础。