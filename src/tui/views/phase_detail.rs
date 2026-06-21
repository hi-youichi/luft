//! 阶段明细面板——左右分栏：左侧智能体列表 + 右侧选中智能体实时信息。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::tui::state::{AgentLifecycle, AgentState, AppState, PhaseStatus};

pub fn render_phase_detail(frame: &mut Frame, area: Rect, state: &AppState) {
    let phase_id = match state.phase_detail_focus {
        Some(pid) => pid,
        None => {
            frame.render_widget(Paragraph::new(" 未选中阶段。按 j/k 选择。"), area);
            return;
        }
    };

    let phase = match state.phases.get(phase_id as usize) {
        Some(p) => p,
        None => return,
    };

    // 布局：顶部阶段信息(3行) + 左右分栏(主体) + 底部提示(1行)
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    // 阶段头部信息
    let filled = if phase.planned > 0 {
        phase.completed * 10 / phase.planned
    } else {
        0
    };
    let bar = "█".repeat(filled) + &"░".repeat(10 - filled);
    let status_str = match phase.status {
        PhaseStatus::Pending => "待处理",
        PhaseStatus::Running => "运行中",
        PhaseStatus::Done => "完成",
        PhaseStatus::Failed => "失败",
    };
    let header_text = format!(
        "  阶段 {}/{}  {}  {}/{}  [{}]  {}ms",
        phase.phase_id + 1,
        state.phases.len(),
        status_str,
        phase.completed,
        phase.planned,
        bar,
        phase.elapsed_ms,
    );
    frame.render_widget(
        Paragraph::new(header_text)
            .style(Style::default().add_modifier(Modifier::BOLD)),
        outer[0],
    );

    // 左右分栏：左侧智能体列表(40%) + 右侧智能体信息(60%)
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(outer[1]);

    // === 左侧：智能体列表 ===
    let mut agents: Vec<&AgentState> = phase
        .agent_ids
        .iter()
        .filter_map(|id| state.agents.get(id))
        .collect();
    agents.sort_by_key(|a| match a.status {
        AgentLifecycle::Error => 0,
        AgentLifecycle::Running => 1,
        AgentLifecycle::TimedOut => 2,
        AgentLifecycle::Cancelled => 3,
        AgentLifecycle::Ok => 4,
    });

    let selected_idx = state
        .agent_detail_focus
        .and_then(|id| agents.iter().position(|a| a.agent_id == id))
        .unwrap_or(0);

    let list_items: Vec<ListItem> = agents
        .iter()
        .enumerate()
        .map(|(i, agent)| {
            let icon = match agent.status {
                AgentLifecycle::Ok => "✅",
                AgentLifecycle::Error => "❌",
                AgentLifecycle::Running => "▶ ",
                AgentLifecycle::Cancelled => "⊘ ",
                AgentLifecycle::TimedOut => "⏱ ",
            };
            let status_label = match agent.status {
                AgentLifecycle::Ok => "完成",
                AgentLifecycle::Error => "错误",
                AgentLifecycle::Running => "运行中",
                AgentLifecycle::Cancelled => "取消",
                AgentLifecycle::TimedOut => "超时",
            };
            let tokens = agent.tokens.total() / 1000;
            let line1 = format!("{} {} {}", icon, crate::tui::state::short_id(agent.agent_id), status_label);
            let line2 = format!(
                "   {}  {}K",
                agent.model.as_deref().unwrap_or("—"),
                tokens
            );
            let style = if i == selected_idx {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(format!("{}\n{}", line1, line2)).style(style)
        })
        .collect();

    let mut list_state = ListState::default();
    list_state.select(Some(selected_idx));

    frame.render_stateful_widget(
        List::new(list_items)
            .block(Block::default().borders(Borders::ALL).title(" 智能体列表 ")),
        main[0],
        &mut list_state,
    );

    // === 右侧：选中智能体信息 ===
    if let Some(agent) = agents.get(selected_idx) {
        render_agent_info(frame, main[1], agent);
    } else {
        frame.render_widget(
            Paragraph::new("  无智能体")
                .block(Block::default().borders(Borders::ALL).title(" 智能体信息 ")),
            main[1],
        );
    }

    // 底部提示
    frame.render_widget(
        Paragraph::new(" j/k 选择智能体   Enter 全屏明细   Esc 返回仪表盘 "),
        outer[2],
    );
}

/// 渲染右侧智能体信息面板。
fn render_agent_info(frame: &mut Frame, area: Rect, agent: &AgentState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(0)])
        .split(area);

    // 头部信息
    let status_badge = match agent.status {
        AgentLifecycle::Ok => "[完成]",
        AgentLifecycle::Error => "[错误]",
        AgentLifecycle::Running => "[运行中]",
        AgentLifecycle::Cancelled => "[已取消]",
        AgentLifecycle::TimedOut => "[超时]",
    };
    let header = format!(
        " {} {}\n 模型: {}  Token: {}K (in {} / out {})\n 耗时: {}ms",
        status_badge,
        crate::tui::state::short_id(agent.agent_id),
        agent.model.as_deref().unwrap_or("—"),
        agent.tokens.total() / 1000,
        agent.tokens.input / 1000,
        agent.tokens.output / 1000,
        agent.elapsed_ms,
    );
    frame.render_widget(
        Paragraph::new(header)
            .block(Block::default().borders(Borders::ALL).title(" 智能体信息 ")),
        chunks[0],
    );

    // 输出流 + 工具调用（上下分割）
    let body = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(chunks[1]);

    // 输出流
    let output: String = agent
        .output_buffer
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let output_height = body[0].height.saturating_sub(2) as usize;
    let total_lines = agent.output_buffer.len();
    let scroll = total_lines.saturating_sub(output_height) as u16;
    frame.render_widget(
        Paragraph::new(output)
            .scroll((scroll, 0))
            .block(Block::default().borders(Borders::ALL).title(" 输出流 ")),
        body[0],
    );

    // 工具调用
    let tool_items: Vec<ListItem> = agent
        .tool_calls
        .iter()
        .map(|tc| ListItem::new(format!("⚙ {} — {}", tc.name, tc.summary)))
        .collect();
    let file_items: Vec<ListItem> = agent
        .file_edits
        .iter()
        .map(|fe| ListItem::new(format!("✎ {}", fe.path)))
        .collect();

    let tool_area = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(body[1]);

    frame.render_widget(
        List::new(tool_items)
            .block(Block::default().borders(Borders::ALL).title(" 工具调用 ")),
        tool_area[0],
    );
    frame.render_widget(
        List::new(file_items)
            .block(Block::default().borders(Borders::ALL).title(" 文件编辑 ")),
        tool_area[1],
    );
}
