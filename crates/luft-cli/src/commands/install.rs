use crate::install::{InstallError, Installer};
use anyhow::Result;

/// 执行 Luft 一键安装
pub fn run_install() -> Result<()> {
    match Installer::install_all() {
        Ok(summary) => {
            print_installation_summary(&summary);
            Ok(())
        }
        Err(InstallError::NoExternalAgentsFound) => {
            eprintln!("❌ 错误: 未检测到任何外部 Agent");
            eprintln!("{}", Installer::get_installation_suggestions());
            Err(anyhow::anyhow!("未检测到任何外部 Agent"))
        }
        Err(err) => {
            eprintln!("❌ 安装过程中发生错误: {}", err);
            Err(anyhow::anyhow!("安装失败: {}", err))
        }
    }
}

/// 打印安装摘要
fn print_installation_summary(summary: &crate::install::InstallSummary) {
    eprintln!();
    eprintln!("检测到的 Agent:");
    for agent in &summary.detected_agents {
        let status = if agent.needs_external_installation() {
            "✅"
        } else {
            "🔧"
        };
        eprintln!("  {} {}", status, agent.display_name());
    }

    eprintln!();
    eprintln!("安装摘要:");
    eprintln!("- 桥接安装: {} 个", summary.bridges_installed.len());
    eprintln!(
        "- MCP 配置: {}",
        if summary.mcp_configured {
            "完成"
        } else {
            "不适用"
        }
    );
    eprintln!("- 耗时: {:.2} 秒", summary.installation_time.as_secs_f64());

    if !summary.bridges_installed.is_empty() {
        eprintln!();
        eprintln!("技能已安装到:");
        for bridge in &summary.bridges_installed {
            eprintln!(
                "  - {} ({} 个技能)",
                bridge.target_dir.display(),
                bridge.skills_count
            );
        }
    }

    eprintln!();
    eprintln!("后续步骤:");
    eprintln!("1. 运行 'luft backend list' 查看所有后端");
    eprintln!("2. 运行 'luft backend info' 查看详细后端信息");
    if summary.mcp_configured {
        eprintln!("3. 运行 'luft mcp serve' 启动 MCP 服务器");
    }
}
