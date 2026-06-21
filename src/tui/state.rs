//! TUI 全局状态——所有投影状态类型定义。
//!
//! `AppState` 由渲染循环唯一拥有，无锁。事件通过 `apply()` 方法投影。

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use crate::core::contract::event::{AgentEvent, LogLevel, ProgressDelta, RunStatus};
use crate::core::contract::finding::Finding;
use crate::core::contract::ids::{AgentId, PhaseId, RunId, TokenUsage};

use super::event_bridge::TuiMsg;

// ---------------------------------------------------------------------------
// 运行级状态
// ---------------------------------------------------------------------------

/// TUI 全局状态——由渲染循环唯一拥有，无锁。
pub struct AppState {
    // === 运行级状态 ===
    pub run_id: Option<RunId>,
    pub run_status: Option<RunStatus>,
    pub task: String,
    pub run_started: Option<Instant>,
    pub total_tokens: TokenUsage,
    pub budget: Option<Budget>,

    // === 阶段状态 ===
    pub phases: Vec<PhaseState>,
    pub current_phase: Option<PhaseId>,

    // === 智能体状态 ===
    pub agents: HashMap<AgentId, AgentState>,

    // === 时间线（环形缓冲） ===
    pub timeline: VecDeque<TimelineEntry>,

    // === 发现 ===
    pub findings: Vec<Finding>,

    // === 日志（环形缓冲） ===
    pub logs: VecDeque<LogEntry>,

    // === 视图导航 ===
    pub active_view: ActiveView,
    pub phase_detail_focus: Option<PhaseId>,
    pub agent_detail_focus: Option<AgentId>,

    // === 脏标记 ===
    pub dirty_views: DirtyFlags,

    // === 统计 ===
    pub events_received: u64,
    pub events_skipped_lag: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            run_id: None,
            run_status: None,
            task: String::new(),
            run_started: None,
            total_tokens: TokenUsage::default(),
            budget: None,
            phases: Vec::new(),
            current_phase: None,
            agents: HashMap::new(),
            timeline: VecDeque::new(),
            findings: Vec::new(),
            logs: VecDeque::new(),
            active_view: ActiveView::Dashboard,
            phase_detail_focus: None,
            agent_detail_focus: None,
            dirty_views: DirtyFlags::all(),
            events_received: 0,
            events_skipped_lag: 0,
        }
    }
}

/// 运行预算（来自 `BudgetSet` 事件）。
pub struct Budget {
    pub time_limit_ms: Option<u64>,
    pub max_rounds: Option<u32>,
}

// ---------------------------------------------------------------------------
// 脏标记
// ---------------------------------------------------------------------------

pub struct DirtyFlags {
    pub dashboard: bool,
    pub phase_detail: bool,
    pub agent_detail: bool,
    pub timeline: bool,
    pub findings: bool,
    pub log_viewer: bool,
    pub status_bar: bool,
}

impl DirtyFlags {
    pub fn all() -> Self {
        Self {
            dashboard: true,
            phase_detail: true,
            agent_detail: true,
            timeline: true,
            findings: true,
            log_viewer: true,
            status_bar: true,
        }
    }

    pub fn any(&self) -> bool {
        self.dashboard
            || self.phase_detail
            || self.agent_detail
            || self.timeline
            || self.findings
            || self.log_viewer
            || self.status_bar
    }

    pub fn clear(&mut self) {
        self.dashboard = false;
        self.phase_detail = false;
        self.agent_detail = false;
        self.timeline = false;
        self.findings = false;
        self.log_viewer = false;
        self.status_bar = false;
    }
}

// ---------------------------------------------------------------------------
// 阶段状态
// ---------------------------------------------------------------------------

/// 单个阶段的投影状态。
pub struct PhaseState {
    pub phase_id: PhaseId,
    pub label: String,
    pub planned: usize,
    pub completed: usize,
    pub failed: usize,
    pub active: usize,
    pub status: PhaseStatus,
    pub started_at: Option<Instant>,
    pub elapsed_ms: u64,
    pub agent_ids: Vec<AgentId>,
    pub error_summary: Option<String>,
}

impl Default for PhaseState {
    fn default() -> Self {
        Self {
            phase_id: 0,
            label: String::new(),
            planned: 0,
            completed: 0,
            failed: 0,
            active: 0,
            status: PhaseStatus::Pending,
            started_at: None,
            elapsed_ms: 0,
            agent_ids: Vec::new(),
            error_summary: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhaseStatus {
    Pending,
    Running,
    Done,
    Failed,
}

// ---------------------------------------------------------------------------
// 智能体状态
// ---------------------------------------------------------------------------

/// 单个智能体的投影状态。
pub struct AgentState {
    pub agent_id: AgentId,
    pub phase_id: PhaseId,
    pub model: Option<String>,
    pub prompt_preview: String,
    pub status: AgentLifecycle,
    pub tokens: TokenUsage,
    pub elapsed_ms: u64,
    pub started_at: Option<Instant>,
    pub output_buffer: VecDeque<String>,
    pub tool_calls: Vec<ToolCallEntry>,
    pub file_edits: Vec<FileEditEntry>,
    pub retry_count: u32,
    pub last_error: Option<String>,
}

impl Default for AgentState {
    fn default() -> Self {
        Self {
            agent_id: RunId::nil(),
            phase_id: 0,
            model: None,
            prompt_preview: String::new(),
            status: AgentLifecycle::Running,
            tokens: TokenUsage::default(),
            elapsed_ms: 0,
            started_at: None,
            output_buffer: VecDeque::new(),
            tool_calls: Vec::new(),
            file_edits: Vec::new(),
            retry_count: 0,
            last_error: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentLifecycle {
    Running,
    Ok,
    Error,
    Cancelled,
    TimedOut,
}

pub struct ToolCallEntry {
    pub name: String,
    pub summary: String,
    pub timestamp: Instant,
}

pub struct FileEditEntry {
    pub path: String,
    pub timestamp: Instant,
}

// ---------------------------------------------------------------------------
// 时间线 & 日志
// ---------------------------------------------------------------------------

/// 格式化后的时间线条目。
pub struct TimelineEntry {
    pub timestamp: Instant,
    pub category: EventCategory,
    pub icon: char,
    pub formatted: String,
    pub agent_id: Option<AgentId>,
    pub phase_id: Option<PhaseId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventCategory {
    Run,
    Phase,
    Agent,
    AgentProgress,
    Log,
    Pipeline,
    Converge,
    Parallel,
    Workflow,
    Budget,
    Report,
    System,
}

pub struct LogEntry {
    pub timestamp: Instant,
    pub level: LogLevel,
    pub agent_id: Option<AgentId>,
    pub msg: String,
}

// ---------------------------------------------------------------------------
// 视图枚举
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActiveView {
    Dashboard,
    Timeline,
    Findings,
    LogViewer,
    PhaseDetail,
    AgentDetail,
}

// ---------------------------------------------------------------------------
// 事件投影
// ---------------------------------------------------------------------------

impl AppState {
    /// 将 TuiMsg（封装的 AgentEvent 或通道控制消息）投影到状态。
    pub fn apply(&mut self, msg: TuiMsg) {
        match msg {
            TuiMsg::Event(evt) => self.apply_event(evt),
            TuiMsg::Lagged(n) => {
                self.events_skipped_lag += n;
                self.push_timeline(
                    EventCategory::System,
                    '⚠',
                    format!("⚠ 广播延迟：{} 个事件被跳过", n),
                );
                self.dirty_views.status_bar = true;
            }
            TuiMsg::Closed => {
                self.push_timeline(
                    EventCategory::System,
                    '◆',
                    "事件流已关闭".to_string(),
                );
                self.dirty_views.status_bar = true;
            }
        }
    }

    fn apply_event(&mut self, evt: AgentEvent) {
        self.events_received += 1;
        match evt {
            AgentEvent::RunStarted { run_id, task, .. } => {
                self.run_id = Some(run_id);
                self.task = task.clone();
                self.run_started = Some(Instant::now());
                self.run_status = Some(RunStatus::Completed);
                self.push_timeline(EventCategory::Run, '▶', format!("运行开始：{}", task));
                self.dirty_views = DirtyFlags::all();
            }

            AgentEvent::PhaseStarted {
                phase_id,
                label,
                planned,
                ..
            } => {
                self.ensure_phase_capacity(phase_id as usize);
                let phase = &mut self.phases[phase_id as usize];
                phase.phase_id = phase_id;
                phase.label = label.clone();
                phase.planned = planned;
                phase.status = PhaseStatus::Running;
                phase.started_at = Some(Instant::now());
                self.current_phase = Some(phase_id);
                self.push_timeline(
                    EventCategory::Phase,
                    '▸',
                    format!("阶段 {}：{}（计划 {} 个）", phase_id, label, planned),
                );
                self.dirty_views.dashboard = true;
                self.dirty_views.timeline = true;
                self.dirty_views.status_bar = true;
            }

            AgentEvent::AgentStarted {
                agent_id,
                phase_id,
                prompt_preview,
                model,
                ..
            } => {
                if let Some(p) = self.phases.get_mut(phase_id as usize) {
                    p.active += 1;
                    p.agent_ids.push(agent_id);
                }
                let preview: String = prompt_preview.chars().take(200).collect();
                self.agents.insert(
                    agent_id,
                    AgentState {
                        agent_id,
                        phase_id,
                        model,
                        prompt_preview: preview,
                        status: AgentLifecycle::Running,
                        started_at: Some(Instant::now()),
                        ..Default::default()
                    },
                );
                let short = short_id(agent_id);
                self.push_timeline(
                    EventCategory::Agent,
                    '●',
                    format!("智能体 {} 已启动", short),
                );
                self.dirty_views.dashboard = true;
                self.dirty_views.timeline = true;
                self.dirty_views.status_bar = true;
                if self.agent_detail_focus == Some(agent_id) {
                    self.dirty_views.agent_detail = true;
                }
            }

            AgentEvent::AgentProgress { agent_id, delta, .. } => {
                if let Some(agent) = self.agents.get_mut(&agent_id) {
                    match delta {
                        ProgressDelta::Message { text } => {
                            for line in text.lines() {
                                agent.output_buffer.push_back(line.to_string());
                            }
                            while agent.output_buffer.len() > 500 {
                                agent.output_buffer.pop_front();
                            }
                        }
                        ProgressDelta::ToolCall { name, summary } => {
                            agent.tool_calls.push(ToolCallEntry {
                                name,
                                summary,
                                timestamp: Instant::now(),
                            });
                        }
                        ProgressDelta::FileEdit { path } => {
                            agent.file_edits.push(FileEditEntry {
                                path: path.display().to_string(),
                                timestamp: Instant::now(),
                            });
                        }
                        ProgressDelta::Tokens { usage } => {
                            agent.tokens = agent.tokens + usage;
                            self.total_tokens = self.total_tokens + usage;
                        }
                    }
                }
                if self.agent_detail_focus == Some(agent_id) {
                    self.dirty_views.agent_detail = true;
                }
                self.dirty_views.status_bar = true;
            }

            AgentEvent::AgentDone {
                agent_id,
                status,
                tokens,
                elapsed_ms,
                ..
            } => {
                if let Some(agent) = self.agents.get_mut(&agent_id) {
                    agent.status = match status {
                        crate::core::contract::backend::AgentStatus::Ok => AgentLifecycle::Ok,
                        crate::core::contract::backend::AgentStatus::Error => AgentLifecycle::Error,
                        crate::core::contract::backend::AgentStatus::Cancelled => {
                            AgentLifecycle::Cancelled
                        }
                        crate::core::contract::backend::AgentStatus::TimedOut => {
                            AgentLifecycle::TimedOut
                        }
                    };
                    agent.tokens = agent.tokens + tokens;
                    agent.elapsed_ms = elapsed_ms;
                }
                if let Some(pid) = self.agents.get(&agent_id).map(|a| a.phase_id) {
                    if let Some(phase) = self.phases.get_mut(pid as usize) {
                        phase.active = phase.active.saturating_sub(1);
                        match status {
                            crate::core::contract::backend::AgentStatus::Ok => phase.completed += 1,
                            _ => phase.failed += 1,
                        }
                    }
                }
                let short = short_id(agent_id);
                let icon = match status {
                    crate::core::contract::backend::AgentStatus::Ok => '✓',
                    crate::core::contract::backend::AgentStatus::Error => '✗',
                    _ => '⚠',
                };
                self.push_timeline(
                    EventCategory::Agent,
                    icon,
                    format!(
                        "智能体 {}：{:?}（{}ms, {} tok）",
                        short,
                        status,
                        elapsed_ms,
                        tokens.total()
                    ),
                );
                self.dirty_views.dashboard = true;
                self.dirty_views.timeline = true;
                self.dirty_views.status_bar = true;
                if self.agent_detail_focus == Some(agent_id) {
                    self.dirty_views.agent_detail = true;
                }
            }

            AgentEvent::PhaseDone {
                phase_id, ok, failed, ..
            } => {
                if let Some(phase) = self.phases.get_mut(phase_id as usize) {
                    phase.status =
                        if failed > 0 { PhaseStatus::Failed } else { PhaseStatus::Done };
                    phase.elapsed_ms = phase
                        .started_at
                        .map(|s| s.elapsed().as_millis() as u64)
                        .unwrap_or(0);
                    phase.completed = ok;
                    phase.failed = failed;
                }
                self.push_timeline(
                    EventCategory::Phase,
                    '◂',
                    format!("阶段 {} 完成：{} 成功，{} 失败", phase_id, ok, failed),
                );
                self.dirty_views.dashboard = true;
                self.dirty_views.timeline = true;
                self.dirty_views.status_bar = true;
            }

            AgentEvent::RunDone {
                status,
                total_tokens,
                ..
            } => {
                self.run_status = Some(status);
                self.total_tokens = total_tokens;
                self.push_timeline(
                    EventCategory::Run,
                    '◆',
                    format!(
                        "运行结束：{:?}（{} tok）",
                        status,
                        total_tokens.total()
                    ),
                );
                self.dirty_views = DirtyFlags::all();
            }

            AgentEvent::Log {
                level,
                agent_id,
                msg,
                ..
            } => {
                self.logs.push_back(LogEntry {
                    timestamp: Instant::now(),
                    level,
                    agent_id,
                    msg: msg.clone(),
                });
                while self.logs.len() > 500 {
                    self.logs.pop_front();
                }
                let icon = match level {
                    LogLevel::Error => '✗',
                    LogLevel::Warn => '⚠',
                    _ => '·',
                };
                self.push_timeline(EventCategory::Log, icon, format!("[{:?}] {}", level, msg));
                self.dirty_views.log_viewer = true;
                self.dirty_views.timeline = true;
            }

            AgentEvent::BudgetSet {
                time_limit_ms,
                max_rounds,
                ..
            } => {
                self.budget = Some(Budget {
                    time_limit_ms,
                    max_rounds,
                });
                self.dirty_views.dashboard = true;
                self.dirty_views.status_bar = true;
            }

            AgentEvent::ReportEmitted { phase_id, .. } => {
                self.push_timeline(
                    EventCategory::Report,
                    '📋',
                    format!("报告已生成（阶段 {}）", phase_id),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::ParallelStarted {
                span_id, count, ..
            } => {
                self.push_timeline(
                    EventCategory::Parallel,
                    '∥',
                    format!("并行 #{:3}：{} 个项目", span_id, count),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::ParallelDone {
                span_id,
                ok,
                failed,
                elapsed_ms,
                ..
            } => {
                self.push_timeline(
                    EventCategory::Parallel,
                    '∥',
                    format!(
                        "并行 #{:3} 完成：{} 成功，{} 失败（{}ms）",
                        span_id, ok, failed, elapsed_ms
                    ),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::WorkflowStarted { span_id, path, .. } => {
                self.push_timeline(
                    EventCategory::Workflow,
                    '▤',
                    format!("子工作流 #{:3}：{}", span_id, path),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::WorkflowDone {
                span_id,
                path,
                elapsed_ms,
                error,
                ..
            } => {
                let status_str = if error.is_some() { "失败" } else { "完成" };
                self.push_timeline(
                    EventCategory::Workflow,
                    '▤',
                    format!(
                        "子工作流 #{:3} {}：{}（{}ms）",
                        span_id, status_str, path, elapsed_ms
                    ),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::PipelineStarted {
                total_stages,
                items,
                ..
            } => {
                self.push_timeline(
                    EventCategory::Pipeline,
                    '═',
                    format!("Pipeline 启动：{} 个阶段，{} 个项目", total_stages, items),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::PipelineStageStarted {
                stage_index,
                label,
                agents_in_stage,
                ..
            } => {
                self.push_timeline(
                    EventCategory::Pipeline,
                    '═',
                    format!(
                        "Pipeline 阶段 {}：{}（{} 个智能体）",
                        stage_index, label, agents_in_stage
                    ),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::PipelineItemDone {
                stage_index,
                item_index,
                status,
                tokens,
                elapsed_ms,
                ..
            } => {
                self.push_timeline(
                    EventCategory::Pipeline,
                    '═',
                    format!(
                        "Pipeline [{},{}]：{:?}（{}ms, {} tok）",
                        stage_index,
                        item_index,
                        status,
                        elapsed_ms,
                        tokens.total()
                    ),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::PipelineDone {
                stages_completed,
                total_ok,
                total_failed,
                ..
            } => {
                self.push_timeline(
                    EventCategory::Pipeline,
                    '═',
                    format!(
                        "Pipeline 完成：{} 阶段，{} 成功，{} 失败",
                        stages_completed, total_ok, total_failed
                    ),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::ConvergeStarted {
                span_id,
                items,
                max_rounds,
                ..
            } => {
                self.push_timeline(
                    EventCategory::Converge,
                    '◎',
                    format!("收敛 #{:3}：{} 个项目，最多 {} 轮", span_id, items, max_rounds),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::ConvergeDone {
                span_id,
                rounds,
                converged,
                surviving,
                elapsed_ms,
                ..
            } => {
                self.push_timeline(
                    EventCategory::Converge,
                    '◎',
                    format!(
                        "收敛 #{:3} 完成：{} 轮，收敛={}，存活={}（{}ms）",
                        span_id, rounds, converged, surviving, elapsed_ms
                    ),
                );
                self.dirty_views.timeline = true;
            }

            AgentEvent::AcpRaw { .. } => {
                // 桥接层已过滤
            }
        }
    }

    fn push_timeline(&mut self, category: EventCategory, icon: char, formatted: String) {
        self.timeline.push_back(TimelineEntry {
            timestamp: Instant::now(),
            category,
            icon,
            formatted,
            agent_id: None,
            phase_id: self.current_phase,
        });
        while self.timeline.len() > 10_000 {
            self.timeline.pop_front();
        }
    }

    fn ensure_phase_capacity(&mut self, idx: usize) {
        while self.phases.len() <= idx {
            self.phases.push(PhaseState::default());
        }
    }
}

/// 将 AgentId/RunId 截取为前 8 个十六进制字符的短格式。
pub fn short_id(id: impl AsRef<uuid::Uuid>) -> String {
    id.as_ref().to_string()[..8].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::backend::AgentStatus;

    #[test]
    fn apply_run_started_sets_run_id_and_task() {
        let mut state = AppState::default();
        let run_id = RunId::now_v7();
        state.apply(TuiMsg::Event(AgentEvent::RunStarted {
            run_id,
            task: "test workflow".into(),
            ts: chrono::Utc::now(),
        }));
        assert_eq!(state.run_id, Some(run_id));
        assert_eq!(state.task, "test workflow");
        assert!(state.run_started.is_some());
    }

    #[test]
    fn apply_phase_started_creates_phase() {
        let mut state = AppState::default();
        state.apply(TuiMsg::Event(AgentEvent::PhaseStarted {
            run_id: RunId::now_v7(),
            phase_id: 0,
            label: "analyze".into(),
            planned: 3,
        }));
        assert_eq!(state.phases.len(), 1);
        assert_eq!(state.phases[0].label, "analyze");
        assert_eq!(state.phases[0].planned, 3);
        assert_eq!(state.phases[0].status, PhaseStatus::Running);
        assert_eq!(state.current_phase, Some(0));
    }

    #[test]
    fn apply_agent_started_and_done_tracks_lifecycle() {
        let mut state = AppState::default();
        let run_id = RunId::now_v7();
        let agent_id = RunId::now_v7();

        // Phase must exist first
        state.apply(TuiMsg::Event(AgentEvent::PhaseStarted {
            run_id,
            phase_id: 0,
            label: "p0".into(),
            planned: 1,
        }));
        state.apply(TuiMsg::Event(AgentEvent::AgentStarted {
            run_id,
            phase_id: 0,
            agent_id,
            prompt_preview: "do stuff".into(),
            model: Some("gpt-4".into()),
        }));

        assert_eq!(state.agents.len(), 1);
        assert_eq!(state.agents[&agent_id].status, AgentLifecycle::Running);
        assert_eq!(state.phases[0].active, 1);

        state.apply(TuiMsg::Event(AgentEvent::AgentDone {
            run_id,
            agent_id,
            status: AgentStatus::Ok,
            tokens: TokenUsage {
                input: 100,
                output: 50,
                ..Default::default()
            },
            elapsed_ms: 5000,
        }));

        assert_eq!(state.agents[&agent_id].status, AgentLifecycle::Ok);
        assert_eq!(state.agents[&agent_id].elapsed_ms, 5000);
        assert_eq!(state.phases[0].active, 0);
        assert_eq!(state.phases[0].completed, 1);
    }

    #[test]
    fn apply_agent_progress_message_accumulates_output() {
        let mut state = AppState::default();
        let run_id = RunId::now_v7();
        let agent_id = RunId::now_v7();

        state.apply(TuiMsg::Event(AgentEvent::PhaseStarted {
            run_id,
            phase_id: 0,
            label: "p0".into(),
            planned: 1,
        }));
        state.apply(TuiMsg::Event(AgentEvent::AgentStarted {
            run_id,
            phase_id: 0,
            agent_id,
            prompt_preview: "".into(),
            model: None,
        }));
        state.apply(TuiMsg::Event(AgentEvent::AgentProgress {
            run_id,
            agent_id,
            delta: ProgressDelta::Message {
                text: "line1\nline2".into(),
            },
        }));

        assert_eq!(state.agents[&agent_id].output_buffer.len(), 2);
    }

    #[test]
    fn apply_run_done_sets_final_status() {
        let mut state = AppState::default();
        let run_id = RunId::now_v7();
        state.apply(TuiMsg::Event(AgentEvent::RunDone {
            run_id,
            status: RunStatus::Completed,
            total_tokens: TokenUsage {
                input: 1000,
                output: 500,
                ..Default::default()
            },
            report: serde_json::json!({"ok": true}),
        }));
        assert_eq!(state.run_status, Some(RunStatus::Completed));
        assert_eq!(state.total_tokens.total(), 1500);
    }

    #[test]
    fn apply_log_event_appends_to_log_buffer() {
        let mut state = AppState::default();
        let run_id = RunId::now_v7();
        state.apply(TuiMsg::Event(AgentEvent::Log {
            run_id,
            agent_id: None,
            level: LogLevel::Warn,
            msg: "something odd".into(),
        }));
        assert_eq!(state.logs.len(), 1);
        assert_eq!(state.logs[0].level, LogLevel::Warn);
    }

    #[test]
    fn timeline_grows_with_events() {
        let mut state = AppState::default();
        let run_id = RunId::now_v7();

        state.apply(TuiMsg::Event(AgentEvent::RunStarted {
            run_id,
            task: "test".into(),
            ts: chrono::Utc::now(),
        }));
        state.apply(TuiMsg::Event(AgentEvent::PhaseStarted {
            run_id,
            phase_id: 0,
            label: "p0".into(),
            planned: 1,
        }));

        assert!(state.timeline.len() >= 2);
    }

    #[test]
    fn dirty_flags_clear_after_clear() {
        let mut flags = DirtyFlags::all();
        assert!(flags.any());
        flags.clear();
        assert!(!flags.any());
    }

    #[test]
    fn short_id_truncates_to_8_chars() {
        let id = RunId::now_v7();
        let short = short_id(id);
        assert_eq!(short.len(), 8);
    }

    #[test]
    fn apply_lagged_increments_skip_count() {
        let mut state = AppState::default();
        state.apply(TuiMsg::Lagged(5));
        assert_eq!(state.events_skipped_lag, 5);
    }

    #[test]
    fn apply_closed_adds_timeline_entry() {
        let mut state = AppState::default();
        let initial_len = state.timeline.len();
        state.apply(TuiMsg::Closed);
        assert_eq!(state.timeline.len(), initial_len + 1);
    }

    #[test]
    fn timeline_ring_buffer_caps_at_10000() {
        let mut state = AppState::default();
        for i in 0..10_001 {
            state.push_timeline(
                EventCategory::System,
                '·',
                format!("entry {}", i),
            );
        }
        assert_eq!(state.timeline.len(), 10_000);
    }

    #[test]
    fn log_ring_buffer_caps_at_500() {
        let mut state = AppState::default();
        let run_id = RunId::now_v7();
        for i in 0..501 {
            state.apply(TuiMsg::Event(AgentEvent::Log {
                run_id,
                agent_id: None,
                level: LogLevel::Info,
                msg: format!("log {}", i),
            }));
        }
        assert_eq!(state.logs.len(), 500);
    }
}
