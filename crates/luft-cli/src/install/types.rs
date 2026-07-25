use std::path::PathBuf;
use std::time::Duration;

/// Agent 类型枚举
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum AgentType {
    /// Mock - Luft 内建，无需检测
    Mock,
    /// Codex - @agentclientprotocol/codex-acp
    Codex,
    /// Opencode - OpenCode Agent
    Opencode,
    /// Claude - Claude Code
    Claude,
    /// Custom - 自定义 Agent (为未来扩展预留)
    Custom(String),
    /// FutureExtension - 未来的扩展 Agent (预留)
    #[serde(skip)]
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

    pub fn needs_external_installation(&self) -> bool {
        !matches!(self, AgentType::Mock | AgentType::FutureExtension)
    }
}

/// 安装摘要
#[derive(Debug, Clone, serde::Serialize)]
pub struct InstallSummary {
    pub detected_agents: Vec<AgentType>,
    pub bridges_installed: Vec<BridgeInstallResult>,
    pub mcp_configured: bool,
    pub installation_time: Duration,
}

/// 桥接安装结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct BridgeInstallResult {
    pub agent_type: Vec<AgentType>,
    pub target_dir: PathBuf,
    pub skills_count: usize,
}

/// 技能安装结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct SkillInstallResult {
    pub target_dir: PathBuf,
    pub skills_count: usize,
}

/// Agent 检测结果
#[derive(Debug, Clone)]
pub struct AgentDetectionResult {
    pub agent_type: AgentType,
    pub is_available: bool,
    pub skill_directory: PathBuf,
    pub supports_mcp: bool,
    pub detector_name: String,
}

/// MCP 配置结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpConfigResult {
    Configured,
    NotSupported,
    Failed(String),
}

/// 安装错误类型
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("未检测到任何外部 Agent")]
    NoExternalAgentsFound,
    
    #[error("Agent 检测失败: {0}")]
    AgentDetection(String),
    
    #[error("桥接安装失败: {0}")]
    BridgeInstallation(String),
    
    #[error("技能复制失败: {0}")]
    SkillCopy(String),
    
    #[error("MCP 配置失败: {0}")]
    McpConfiguration(String),
    
    #[error("无法找到用户主目录")]
    HomeDirNotFound,
    
    #[error("技能源目录不存在: {0}")]
    SkillSourceNotFound(PathBuf),
    
    #[error("安装验证失败: {0}")]
    VerificationFailed(String),
    
    #[error("不支持的 Agent 类型")]
    UnsupportedAgentType,
    
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
    
    #[error("其他错误: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, InstallError>;