//! 底部状态栏——运行摘要信息。

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::tui::state::{AgentLifecycle, AppState};

pub fn render_status_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    let elapsed = state
        .run_started
        .map(|s| s.elapsed())
        .unwrap_or_default();

    let active_agents = state
        .agents
        .values()
        .filter(|a| a.status == AgentLifecycle::Running)
        .count();
    let total_agents = state.agents.len();

    let phase_str = match state.current_phase {
        Some(pid) => format!("阶段 {}/{}", pid + 1, state.phases.len()),
        None => "阶段 —".to_string(),
    };

    let token_str = format!("{}K", state.total_tokens.total() / 1000);

    let lag_str = if state.events_skipped_lag > 0 {
        format!(" ⚠ {} 延迟跳过", state.events_skipped_lag)
    } else {
        String::new()
    };

    let task_preview: String = state.task.chars().take(20).collect();

    let text = format!(
        " {} | {} | 智能体 {}/{} | {} | {:02}:{:02}:{:02}{}",
        task_preview,
        phase_str,
        active_agents,
        total_agents,
        token_str,
        elapsed.as_secs() / 3600,
        (elapsed.as_secs() % 3600) / 60,
        elapsed.as_secs() % 60,
        lag_str,
    );

    frame.render_widget(
        Paragraph::new(text).style(Style::default().bg(Color::DarkGray).fg(Color::White)),
        area,
    );
}
