//! TUI 模块——终端交互式监控面板。
//!
//! TUI 作为纯消费者订阅 `broadcast::Sender<AgentEvent>`，将事件投影到本地
//! `AppState`，并通过 ratatui 渲染循环展示。TUI 不回写任何数据、不影响调度
//! 器行为。
//!
//! 架构要点：
//! - broadcast → mpsc 桥接（避免 `Arc<RwLock>` 死锁风险）
//! - 单线程状态所有权（渲染循环唯一拥有 AppState）
//! - 脏标记驱动渲染（无事件时不重绘）

pub mod app;
pub mod event_bridge;
pub mod input;
pub mod state;
pub mod theme;
pub mod views;
pub mod widgets;

use std::io::{self, stdout};
use std::path::Path;

use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::{broadcast, mpsc};

use crate::core::contract::event::AgentEvent;
use crate::core::contract::ids::RunId;

use app::App;
use event_bridge::spawn_bridge;
use input::spawn_input_reader;
use state::AppState;

/// TUI 入口：消费 broadcast Receiver，实时展示运行状态。
pub async fn run_with_events(
    broadcast_rx: broadcast::Receiver<AgentEvent>,
) -> anyhow::Result<()> {
    let mut terminal = setup_terminal()?;
    let (event_tx, event_rx) = mpsc::channel(512);
    let (input_tx, input_rx) = mpsc::channel(64);

    let bridge = spawn_bridge(broadcast_rx, event_tx);
    let input = spawn_input_reader(input_tx);

    let app = App::new(AppState::default(), terminal.backend_mut(), event_rx, input_rx);
    let result = app.run().await;

    restore_terminal(&mut terminal)?;
    input.abort();
    bridge.abort();
    result
}

/// TUI 回放模式：从 events.jsonl 读取历史事件，在 TUI 中回放。
pub async fn run_replay(run_dir_name: &str, base_dir: &Path) -> anyhow::Result<()> {
    let _run_id = RunId::now_v7(); // placeholder——回放不需要真实 run_id
    let events = crate::service::query::get_events(run_dir_name, base_dir)?;

    let mut terminal = setup_terminal()?;
    let (event_tx, event_rx) = mpsc::channel(512);
    let (input_tx, input_rx) = mpsc::channel(64);

    // 回放 task：以适当间隔发送历史事件
    let replay = tokio::spawn(async move {
        for evt in events {
            if matches!(evt, AgentEvent::AcpRaw { .. }) {
                continue;
            }
            if event_tx.send(event_bridge::TuiMsg::Event(evt)).await.is_err() {
                break;
            }
        }
        let _ = event_tx.send(event_bridge::TuiMsg::Closed).await;
    });

    let input = spawn_input_reader(input_tx);
    let app = App::new(AppState::default(), terminal.backend_mut(), event_rx, input_rx);
    let result = app.run().await;

    restore_terminal(&mut terminal)?;
    let _ = replay.await;
    input.abort();
    result
}

/// RAII 守卫：确保终端在任何情况下（panic、错误返回）恢复原始状态。
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(stdout(), EnterAlternateScreen)?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), LeaveAlternateScreen);
    }
}

/// 初始化终端：启用 raw mode + 交替屏。
fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    let _guard = TerminalGuard::enter()?;
    std::mem::forget(_guard); // 由调用方在 restore_terminal 中恢复
    let backend = CrosstermBackend::new(stdout());
    Terminal::new(backend)
}

/// 恢复终端原始状态。
fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}
