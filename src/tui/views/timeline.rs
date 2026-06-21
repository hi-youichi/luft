//! 时间线视图——逆向滚动的事件日志。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, List, ListItem};
use ratatui::Frame;

use crate::tui::state::{AppState, EventCategory};

pub fn render_timeline(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // 时间线列表
            Constraint::Length(1), // 提示栏
        ])
        .split(area);

    // 逆向显示（最新在底部）
    let items: Vec<ListItem> = state
        .timeline
        .iter()
        .rev()
        .take(500) // 限制渲染条数
        .map(|entry| {
            let color = category_color(entry.category);
            ListItem::from(format!(" {} {}", entry.icon, entry.formatted))
                .style(Style::default().fg(color))
        })
        .collect();

    frame.render_widget(
        List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" 时间线 ({}) ", state.timeline.len())),
            ),
        chunks[0],
    );

    frame.render_widget(
        ratatui::widgets::Paragraph::new(" Tab 切换视图   Esc 返回仪表盘 "),
        chunks[1],
    );
}

fn category_color(cat: EventCategory) -> Color {
    match cat {
        EventCategory::Run => Color::Magenta,
        EventCategory::Phase => Color::Blue,
        EventCategory::Agent => Color::Green,
        EventCategory::AgentProgress => Color::LightGreen,
        EventCategory::Log => Color::Gray,
        EventCategory::Pipeline => Color::Cyan,
        EventCategory::Converge => Color::Yellow,
        EventCategory::Parallel => Color::LightBlue,
        EventCategory::Workflow => Color::LightMagenta,
        EventCategory::Budget => Color::DarkGray,
        EventCategory::Report => Color::LightCyan,
        EventCategory::System => Color::Red,
    }
}
