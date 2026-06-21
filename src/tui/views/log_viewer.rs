//! 日志查看器——级别着色的日志流。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, List, ListItem};
use ratatui::Frame;

use crate::core::contract::event::LogLevel;
use crate::tui::state::AppState;

pub fn render_log_viewer(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // 日志列表
            Constraint::Length(1), // 提示栏
        ])
        .split(area);

    let items: Vec<ListItem> = state
        .logs
        .iter()
        .rev()
        .take(500)
        .map(|entry| {
            let color = level_color(entry.level);
            ListItem::from(format!("[{:?}] {}", entry.level, entry.msg))
                .style(Style::default().fg(color))
        })
        .collect();

    frame.render_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" 日志 ({}) ", state.logs.len())),
            ),
        chunks[0],
    );

    frame.render_widget(
        ratatui::widgets::Paragraph::new(" Tab 切换视图   Esc 返回仪表盘 "),
        chunks[1],
    );
}

fn level_color(level: LogLevel) -> Color {
    match level {
        LogLevel::Error => Color::Red,
        LogLevel::Warn => Color::Yellow,
        LogLevel::Info => Color::Green,
        LogLevel::Debug => Color::Gray,
        LogLevel::Trace => Color::DarkGray,
    }
}
