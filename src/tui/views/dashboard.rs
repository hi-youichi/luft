//! 仪表盘视图——阶段卡片网格 + Token 进度 + 智能体概览。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::tui::state::{
    AgentLifecycle, AppState, PhaseState, PhaseStatus,
};

pub fn render_dashboard(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),  // 阶段卡片网格
            Constraint::Length(3),  // Token 进度条
            Constraint::Min(0),     // 智能体概览列表
        ])
        .split(area);

    render_phase_grid(frame, chunks[0], state);
    render_token_bar(frame, chunks[1], state);
    render_agent_overview(frame, chunks[2], state);
}

fn render_phase_grid(frame: &mut Frame, area: Rect, state: &AppState) {
    let phases = &state.phases;
    if phases.is_empty() {
        frame.render_widget(
            Paragraph::new("等待阶段开始…")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL).title(" 阶段 ")),
            area,
        );
        return;
    }

    // 计算每行可容纳的卡片数（每个卡片最小宽度 14）
    let cols = (area.width as usize / 14).max(1).min(phases.len());
    let rows = (phases.len() + cols - 1) / cols;

    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Percentage((100 / cols) as u16); cols])
        .split(area);

    for (i, phase) in phases.iter().enumerate() {
        let row = i / cols;
        let col = i % cols;

        let v_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![Constraint::Length(3); rows])
            .split(h_chunks[col]);

        let is_selected = state.phase_detail_focus == Some(phase.phase_id);
        render_phase_card(frame, v_chunks[row], phase, is_selected);
    }
}

fn render_phase_card(
    frame: &mut Frame,
    area: Rect,
    phase: &PhaseState,
    selected: bool,
) {
    let (icon, color) = match phase.status {
        PhaseStatus::Done => ("✓", Color::Green),
        PhaseStatus::Running => ("▶", Color::Yellow),
        PhaseStatus::Failed => ("✗", Color::Red),
        PhaseStatus::Pending => ("⏳", Color::DarkGray),
    };

    let border_style = if selected {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(format!(" {} {} ", phase.label, icon));

    let content = format!(
        "{}/{}\n{} {}",
        phase.completed + phase.failed,
        phase.planned,
        phase.active,
        if phase.active > 0 { "运行中" } else { "" }
    );

    let paragraph = Paragraph::new(content).style(Style::default().fg(color));

    frame.render_widget(paragraph.block(block), area);
}

fn render_token_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    let total = state.total_tokens.total();
    let input_pct = if total > 0 {
        state.total_tokens.input as u16 * 100 / total as u16
    } else {
        0
    };
    let output_pct = if total > 0 {
        state.total_tokens.output as u16 * 100 / total as u16
    } else {
        0
    };

    let text = format!(
        "Token: {}K total  |  Input: {}%  Output: {}%  |  Cache R: {}  W: {}",
        total / 1000,
        input_pct,
        output_pct,
        state.total_tokens.cache_read,
        state.total_tokens.cache_write,
    );

    frame.render_widget(
        Paragraph::new(text)
            .style(Style::default().fg(Color::Cyan))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Token 用量 "),
            ),
        area,
    );
}

fn render_agent_overview(frame: &mut Frame, area: Rect, state: &AppState) {
    let mut agents: Vec<&_> = state.agents.values().collect();
    // 按优先级排序：错误 > 运行中 > 完成
    agents.sort_by_key(|a| match a.status {
        AgentLifecycle::Error => 0,
        AgentLifecycle::Running => 1,
        AgentLifecycle::TimedOut => 2,
        AgentLifecycle::Cancelled => 3,
        AgentLifecycle::Ok => 4,
    });

    let lines: Vec<String> = agents
        .iter()
        .take(20) // 限制显示行数
        .map(|a| {
            let icon = match a.status {
                AgentLifecycle::Ok => "✓",
                AgentLifecycle::Error => "✗",
                AgentLifecycle::Running => "▶",
                AgentLifecycle::Cancelled => "⊘",
                AgentLifecycle::TimedOut => "⏱",
            };
            format!(
                "  {} {}  {:?}  {}ms  {}tok  {}",
                icon,
                &crate::tui::state::short_id(a.agent_id),
                a.status,
                a.elapsed_ms,
                a.tokens.total(),
                a.model.as_deref().unwrap_or("—"),
            )
        })
        .collect();

    let content = if lines.is_empty() {
        "等待智能体启动…".to_string()
    } else {
        lines.join("\n")
    };

    frame.render_widget(
        Paragraph::new(content)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" 智能体 ({}) ", state.agents.len())),
            ),
        area,
    );
}
