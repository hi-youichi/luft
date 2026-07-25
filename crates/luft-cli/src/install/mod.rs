//! Luft 一键安装模块
//!
//! 此模块提供了自动检测已安装的 Agent 并安装 Luft 桥接组件的功能。

pub mod agent_detector;
pub mod installer;
pub mod mcp_setup;
pub mod skill_manager;
pub mod types;

// 重新导出常用类型，方便外部使用
pub use types::{InstallError, InstallSummary};

// 重新导出主要接口
pub use installer::Installer;
