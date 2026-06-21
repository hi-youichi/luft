//! TUI 配色方案与样式常量。

use ratatui::style::{Color, Modifier, Style};

/// 主题色系。
pub struct Theme;

impl Theme {
    pub fn phase_done() -> Style {
        Style::default().fg(Color::Green)
    }
    pub fn phase_running() -> Style {
        Style::default().fg(Color::Yellow)
    }
    pub fn phase_failed() -> Style {
        Style::default().fg(Color::Red)
    }
    pub fn phase_pending() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    pub fn selected() -> Style {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    }
    pub fn unselected_border() -> Style {
        Style::default().fg(Color::DarkGray)
    }
    pub fn status_bar() -> Style {
        Style::default().bg(Color::DarkGray).fg(Color::White)
    }
    pub fn tab_active() -> Style {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    }
    pub fn tab_inactive() -> Style {
        Style::default().fg(Color::Gray)
    }
}
