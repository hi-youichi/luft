//! 发现视图——按严重性排序的结构化发现表格。

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::Frame;

use crate::core::contract::finding::Severity;
use crate::tui::state::AppState;

pub fn render_findings(frame: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // 发现表格
            Constraint::Length(1), // 提示栏
        ])
        .split(area);

    let mut findings = state.findings.clone();
    findings.sort_by(|a, b| b.severity.cmp(&a.severity));

    let rows: Vec<Row> = findings
        .iter()
        .map(|f| {
            let (icon, color) = severity_icon_color(f.severity);
            Row::new(vec![
                Cell::from(format!("{} {:?}", icon, f.severity))
                    .style(Style::default().fg(color)),
                Cell::from(f.kind.clone()),
                Cell::from(f.title.clone()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Percentage(30),
            Constraint::Percentage(50),
        ],
    )
    .header(
        Row::new(vec!["严重性", "类别", "标题"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" 发现 ({}) ", state.findings.len())),
    );

    frame.render_widget(table, chunks[0]);

    frame.render_widget(
        Paragraph::new(" Tab 切换视图   Esc 返回仪表盘 "),
        chunks[1],
    );
}

fn severity_icon_color(sev: Severity) -> (&'static str, Color) {
    match sev {
        Severity::Critical => ("🔴", Color::Red),
        Severity::High => ("🟠", Color::LightRed),
        Severity::Medium => ("🟡", Color::Yellow),
        Severity::Low => ("🟢", Color::Green),
        Severity::Info => ("ℹ", Color::Blue),
    }
}
