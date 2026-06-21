//! 智能体明细视图——输出流 + 工具调用 + 文件编辑双面板。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::tui::state::{AgentLifecycle, AppState};

pub fn render_agent_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let agent_id = match state.agent_detail_focus {
        Some(id) => id,
        None => {
            frame.render_widget(Paragraph::new("未选中智能体。"), area);
            return;
        }
    };

    let agent = match state.agents.get(&agent_id) {
        Some(a) => a,
        None => return,
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),  // 头部信息
            Constraint::Min(0),    // 主内容
            Constraint::Length(1), // 提示栏
        ])
        .split(area);

    // 头部
    let status_badge = match agent.status {
        AgentLifecycle::Ok => "[完成]",
        AgentLifecycle::Error => "[错误]",
        AgentLifecycle::Running => "[运行中]",
        AgentLifecycle::Cancelled => "[已取消]",
        AgentLifecycle::TimedOut => "[超时]",
    };
    let header = format!(
        "← 返回   {} {}\n模型: {}  阶段: {}  耗时: {}ms  Token: {} (in {} / out {})",
        status_badge,
        &crate::tui::state::short_id(agent_id),
        agent.model.as_deref().unwrap_or("—"),
        agent.phase_id,
        agent.elapsed_ms,
        agent.tokens.total(),
        agent.tokens.input,
        agent.tokens.output,
    );
    frame.render_widget(
        Paragraph::new(header)
            .block(Block::default().borders(Borders::ALL).title(" 智能体明细 ")),
        chunks[0],
    );

    // 主内容：水平分割
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(75), // 输出流
            Constraint::Percentage(25), // 工具调用
        ])
        .split(chunks[1]);

    // 输出流
    let output_text: String = agent
        .output_buffer
        .iter()
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");

    let scroll = agent
        .output_buffer
        .len()
        .saturating_sub(h_chunks[0].height as usize - 2) as u16;

    frame.render_widget(
        Paragraph::new(output_text)
            .scroll((scroll, 0))
            .block(Block::default().borders(Borders::ALL).title(" 输出流 ")),
        h_chunks[0],
    );

    // 侧栏：工具调用 + 文件编辑
    let tool_items: Vec<ListItem> = agent
        .tool_calls
        .iter()
        .map(|tc| ListItem::from(format!("⚙ {} — {}", tc.name, tc.summary)))
        .collect();

    let file_items: Vec<ListItem> = agent
        .file_edits
        .iter()
        .map(|fe| ListItem::from(format!("✎ {}", fe.path)))
        .collect();

    let sidebar_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(h_chunks[1]);

    frame.render_widget(
        List::new(tool_items)
            .block(Block::default().borders(Borders::ALL).title(" 工具调用 ")),
        sidebar_chunks[0],
    );
    frame.render_widget(
        List::new(file_items)
            .block(Block::default().borders(Borders::ALL).title(" 文件编辑 ")),
        sidebar_chunks[1],
    );

    // 提示栏
    frame.render_widget(
        Paragraph::new(" Esc 返回阶段明细   H/L 切换智能体   q 退出 "),
        chunks[2],
    );
}
