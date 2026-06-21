//! App 结构体——主渲染循环、终端管理、输入处理。
//!
//! 渲染循环使用 `tokio::select!` 同时消费事件和输入，以 ~30 FPS 的节拍
//! 重绘脏视图。

use std::io::{self, Stdout};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::{Block, Borders, Tabs};
use ratatui::Frame;
use tokio::sync::mpsc;

use crate::core::contract::ids::{AgentId, PhaseId};

use super::input::InputMsg;
use super::state::{
    ActiveView, AgentLifecycle, AppState, DirtyFlags,
};
use super::event_bridge::TuiMsg;

type Backend = CrosstermBackend<Stdout>;

/// 主 App——拥有 AppState 和终端。
pub struct App {
    pub state: AppState,
    event_rx: mpsc::Receiver<TuiMsg>,
    input_rx: mpsc::Receiver<InputMsg>,
    should_quit: bool,
}

impl App {
    pub fn new(
        state: AppState,
        _backend: &mut Backend,
        event_rx: mpsc::Receiver<TuiMsg>,
        input_rx: mpsc::Receiver<InputMsg>,
    ) -> Self {
        Self {
            state,
            event_rx,
            input_rx,
            should_quit: false,
        }
    }

    /// 主渲染循环。调用方负责终端初始化/恢复。
    pub async fn run(mut self) -> anyhow::Result<()> {
        use ratatui::Terminal;

        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;

        let result = self.main_loop(&mut terminal).await;

        terminal.show_cursor()?;
        result
    }

    async fn main_loop(&mut self, terminal: &mut ratatui::Terminal<Backend>) -> anyhow::Result<()> {
        let mut render_timer = tokio::time::interval(Duration::from_millis(33));

        loop {
            if self.should_quit {
                return Ok(());
            }

            tokio::select! {
                biased;

                // 优先级 0：SIGINT（Ctrl+C 在非 raw 模式终端的兜底）
                _ = tokio::signal::ctrl_c() => {
                    self.should_quit = true;
                }

                // 优先级 1：键盘输入
                Some(input) = self.input_rx.recv() => {
                    self.handle_input(input);
                }

                // 优先级 2：事件（可能批量到达）
                Some(msg) = self.event_rx.recv() => {
                    self.state.apply(msg);
                    while let Ok(msg) = self.event_rx.try_recv() {
                        self.state.apply(msg);
                    }
                }

                // 优先级 3：渲染节拍（~30 FPS）
                _ = render_timer.tick() => {
                    if self.state.dirty_views.any() {
                        terminal.draw(|frame| self.render(frame))?;
                        self.state.dirty_views.clear();
                    }
                }
            }
        }
    }

    // --- 输入处理 ---

    fn handle_input(&mut self, input: InputMsg) {
        match input {
            InputMsg::Resize(_, _) => {
                self.state.dirty_views = DirtyFlags::all();
            }
            InputMsg::Key(key) => {
                // 全局快捷键
                match key.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => {
                        self.should_quit = true;
                        return;
                    }
                    KeyCode::Char('c')
                        if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
                    {
                        self.should_quit = true;
                        return;
                    }
                    _ => {}
                }
                // 视图级快捷键
                match self.state.active_view {
                    ActiveView::Dashboard => self.handle_dashboard_input(key),
                    ActiveView::PhaseDetail => self.handle_phase_detail_input(key),
                    ActiveView::AgentDetail => self.handle_agent_detail_input(key),
                    ActiveView::Timeline => self.handle_timeline_input(key),
                    ActiveView::Findings => self.handle_findings_input(key),
                    ActiveView::LogViewer => self.handle_log_viewer_input(key),
                }
            }
        }
    }

    fn handle_dashboard_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab => {
                self.state.active_view = ActiveView::Timeline;
                self.state.dirty_views = DirtyFlags::all();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.navigate_phase(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.navigate_phase(-1);
            }
            KeyCode::Enter => {
                if let Some(phase_id) = self.state.phase_detail_focus {
                    self.state.active_view = ActiveView::PhaseDetail;
                    // 自动选中阶段中第一个智能体
                    if self.state.agent_detail_focus.is_none() {
                        if let Some(phase) = self.state.phases.get(phase_id as usize) {
                            if let Some(first) = phase.agent_ids.first() {
                                self.state.agent_detail_focus = Some(*first);
                            }
                        }
                    }
                    self.state.dirty_views = DirtyFlags::all();
                }
            }
            KeyCode::Char('1') => {
                self.switch_view(ActiveView::Dashboard);
            }
            KeyCode::Char('2') => {
                self.switch_view(ActiveView::Timeline);
            }
            KeyCode::Char('3') => {
                self.switch_view(ActiveView::Findings);
            }
            KeyCode::Char('4') => {
                self.switch_view(ActiveView::LogViewer);
            }
            _ => {}
        }
    }

    fn handle_phase_detail_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.state.active_view = ActiveView::Dashboard;
                self.state.dirty_views = DirtyFlags::all();
            }
            // j/k 在左侧智能体列表中上下移动，右侧信息实时跟随
            KeyCode::Char('j') | KeyCode::Down => {
                self.navigate_phase_agent(1);
                self.state.dirty_views.phase_detail = true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.navigate_phase_agent(-1);
                self.state.dirty_views.phase_detail = true;
            }
            // Enter 进入选中智能体的全屏明细
            KeyCode::Enter => {
                if self.state.agent_detail_focus.is_some() {
                    self.state.active_view = ActiveView::AgentDetail;
                    self.state.dirty_views = DirtyFlags::all();
                }
            }
            _ => {}
        }
    }

    fn handle_agent_detail_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.state.active_view = ActiveView::PhaseDetail;
                self.state.dirty_views = DirtyFlags::all();
            }
            KeyCode::Char('H') => {
                self.navigate_agent(-1);
            }
            KeyCode::Char('L') => {
                self.navigate_agent(1);
            }
            _ => {}
        }
    }

    fn handle_timeline_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab => {
                self.state.active_view = ActiveView::Findings;
                self.state.dirty_views = DirtyFlags::all();
            }
            KeyCode::Esc => {
                self.switch_view(ActiveView::Dashboard);
            }
            _ => {}
        }
    }

    fn handle_findings_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab => {
                self.state.active_view = ActiveView::LogViewer;
                self.state.dirty_views = DirtyFlags::all();
            }
            KeyCode::Esc => {
                self.switch_view(ActiveView::Dashboard);
            }
            _ => {}
        }
    }

    fn handle_log_viewer_input(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Tab => {
                self.state.active_view = ActiveView::Dashboard;
                self.state.dirty_views = DirtyFlags::all();
            }
            KeyCode::Esc => {
                self.switch_view(ActiveView::Dashboard);
            }
            _ => {}
        }
    }

    fn navigate_phase(&mut self, delta: i32) {
        let phases = &self.state.phases;
        if phases.is_empty() {
            return;
        }
        let current = self.state.phase_detail_focus.map(|p| p as i32).unwrap_or(0);
        let next = (current + delta).clamp(0, phases.len() as i32 - 1) as PhaseId;
        self.state.phase_detail_focus = Some(next);
        self.state.dirty_views.dashboard = true;
    }

    /// 在阶段明细的左侧智能体列表中上下移动。
    fn navigate_phase_agent(&mut self, delta: i32) {
        let phase_id = match self.state.phase_detail_focus {
            Some(p) => p,
            None => return,
        };
        let phase = match self.state.phases.get(phase_id as usize) {
            Some(p) => p,
            None => return,
        };
        let agents: Vec<AgentId> = phase.agent_ids.clone();
        if agents.is_empty() {
            return;
        }
        let current_idx = self
            .state
            .agent_detail_focus
            .and_then(|id| agents.iter().position(|a| *a == id))
            .unwrap_or(0) as i32;
        let next_idx = (current_idx + delta).clamp(0, agents.len() as i32 - 1) as usize;
        self.state.agent_detail_focus = Some(agents[next_idx]);
    }

    fn navigate_agent(&mut self, delta: i32) {
        let phase_id = match self.state.phase_detail_focus {
            Some(p) => p,
            None => return,
        };
        let phase = match self.state.phases.get(phase_id as usize) {
            Some(p) => p,
            None => return,
        };
        let agents: Vec<AgentId> = phase.agent_ids.clone();
        if agents.is_empty() {
            return;
        }
        let current_idx = self
            .state
            .agent_detail_focus
            .and_then(|id| agents.iter().position(|a| *a == id))
            .unwrap_or(0) as i32;
        let next_idx = (current_idx + delta).clamp(0, agents.len() as i32 - 1) as usize;
        self.state.agent_detail_focus = Some(agents[next_idx]);
        self.state.dirty_views.agent_detail = true;
    }

    fn switch_view(&mut self, view: ActiveView) {
        self.state.active_view = view;
        self.state.dirty_views = DirtyFlags::all();
    }

    // --- 渲染 ---

    fn render(&self, frame: &mut Frame) {
        let area = frame.area();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // 标签栏
                Constraint::Min(0),    // 主内容
                Constraint::Length(1), // 状态栏
            ])
            .split(area);

        self.render_tab_bar(frame, chunks[0]);

        match self.state.active_view {
            ActiveView::Dashboard => {
                super::views::dashboard::render_dashboard(frame, chunks[1], &self.state)
            }
            ActiveView::PhaseDetail => {
                super::views::phase_detail::render_phase_detail(frame, chunks[1], &self.state)
            }
            ActiveView::AgentDetail => {
                super::views::agent_detail::render_agent_detail(frame, chunks[1], &self.state)
            }
            ActiveView::Timeline => {
                super::views::timeline::render_timeline(frame, chunks[1], &self.state)
            }
            ActiveView::Findings => {
                super::views::findings::render_findings(frame, chunks[1], &self.state)
            }
            ActiveView::LogViewer => {
                super::views::log_viewer::render_log_viewer(frame, chunks[1], &self.state)
            }
        }

        super::widgets::status_bar::render_status_bar(frame, chunks[2], &self.state);
    }

    fn render_tab_bar(&self, frame: &mut Frame, area: Rect) {
        use ratatui::text::Line;

        let tabs = ["1 仪表盘", "2 时间线", "3 发现", "4 日志"];
        let active_idx = match self.state.active_view {
            ActiveView::Dashboard | ActiveView::PhaseDetail | ActiveView::AgentDetail => 0,
            ActiveView::Timeline => 1,
            ActiveView::Findings => 2,
            ActiveView::LogViewer => 3,
        };

        let titles: Vec<Line> = tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                let style = if i == active_idx {
                    super::theme::Theme::tab_active()
                } else {
                    super::theme::Theme::tab_inactive()
                };
                Line::from(*t).style(style)
            })
            .collect();

        let tabs_widget = Tabs::new(titles).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Maestro "),
        );

        frame.render_widget(tabs_widget, area);
    }
}
