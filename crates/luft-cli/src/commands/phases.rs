//! `phases` subcommand: render the planned phase structure of a run.
//!
//! Reads the checkpoint (preferring `workflow_meta`, falling back to events
//! when the script didn't declare a `meta` table) and prints a tree-formatted
//! progress view. `--json` switches to a JSON dump for tooling.

use super::runs_base_dir;
use luft::core::contract::event::AgentEvent;
use luft::service::phases::{PhasesView, PhaseStatus};
use anyhow::Result;
use std::io::Write;

#[derive(Debug, Clone, Copy, Default)]
pub struct PhasesArgs {
    pub json: bool,
}

pub fn phases_cmd(run_dir: String, args: PhasesArgs) -> Result<()> {
    phases_cmd_inner(&mut std::io::stdout(), run_dir, args)
}

pub(crate) fn phases_cmd_inner(
    w: &mut impl Write,
    run_dir: String,
    args: PhasesArgs,
) -> Result<()> {
    let base_dir = runs_base_dir();
    let checkpoint_path = base_dir.join(&run_dir).join("checkpoint.json");
    if !checkpoint_path.exists() {
        anyhow::bail!("run not found or has no checkpoint: {}", run_dir);
    }
    let checkpoint_content = std::fs::read_to_string(&checkpoint_path)
        .map_err(|e| anyhow::anyhow!("failed to read checkpoint: {}", e))?;
    let checkpoint: luft::core::state::RunCheckpoint = serde_json::from_str(&checkpoint_content)
        .map_err(|e| anyhow::anyhow!("failed to parse checkpoint: {}", e))?;

    let events_path = base_dir.join(&run_dir).join("events.jsonl");
    let events = read_events(&events_path);

    let view = luft::service::phases::build_phases_view(&checkpoint, &events);

    if args.json {
        let json = serde_json::to_string_pretty(&view)
            .map_err(|e| anyhow::anyhow!("failed to serialize phases view: {}", e))?;
        writeln!(w, "{}", json)?;
    } else {
        render_phases(w, &view)?;
    }

    Ok(())
}

fn read_events(path: &std::path::Path) -> Vec<AgentEvent> {
    let Ok(content) = std::fs::read_to_string(path) else {
        return vec![];
    };
    content
        .lines()
        .filter_map(|line| serde_json::from_str::<AgentEvent>(line).ok())
        .collect()
}

fn render_phases(w: &mut impl Write, view: &PhasesView) -> std::io::Result<()> {
    let h = &view.run;

    // Header line
    let short_id = format!("{:.8}", h.run_id);
    let elapsed_str = match h.elapsed_secs {
        Some(s) => format_elapsed(s),
        None => "?s".to_string(),
    };
    writeln!(
        w,
        "Run {}  status={:?}  current_phase={}/{}  tokens={}  elapsed={}",
        short_id, h.status, h.current_phase, h.total_phases, h.total_tokens, elapsed_str
    )?;
    writeln!(w, "  Task: {}", h.task)?;
    writeln!(w)?;

    // Phase label alignment width
    let label_w = view
        .phases
        .iter()
        .map(|p| p.label.len())
        .max()
        .unwrap_or(8)
        .max(8);

    let total = view.phases.len();
    for (i, phase) in view.phases.iter().enumerate() {
        let phase_num = i + 1;

        // Phase header line
        match phase.status {
            PhaseStatus::Pending => {
                writeln!(
                    w,
                    "Phase {}/{}  {:<label_w$}  pending",
                    phase_num, total, phase.label,
                    label_w = label_w,
                )?;
            }
            _ => {
                let elapsed_str = match phase.elapsed_secs {
                    Some(s) => format_elapsed(s),
                    None => "?s".to_string(),
                };
                writeln!(
                    w,
                    "Phase {}/{}  {:<label_w$}  ok={} failed={}  {:>10}  {}",
                    phase_num, total, phase.label, phase.ok, phase.failed,
                    phase.status.bracket(), elapsed_str,
                    label_w = label_w,
                )?;
            }
        }

        // Detail line
        if let Some(ref detail) = phase.detail {
            writeln!(w, "  ┊ {}", detail)?;
        }

        // Agent lines
        let agent_count = phase.agents.len();
        for (j, agent) in phase.agents.iter().enumerate() {
            let prefix = if j + 1 == agent_count { "└─" } else { "├─" };
            let tokens_str = match agent.tokens {
                Some(t) => format!("{} tok", t),
                None => "— tok".to_string(),
            };
            if agent.status == "running" {
                let tool_str = agent.tool_count
                    .map(|c| format!("tools={}", c))
                    .unwrap_or_default();
                let msg_str = agent.last_message
                    .as_ref()
                    .filter(|m| !m.is_empty())
                    .map(|m| {
                        let preview: String = m.chars().take(60).collect();
                        format!("│ {}", preview)
                    });
                if let Some(msg) = &msg_str {
                    writeln!(
                        w,
                        "  {} {:<8}  {:<10}  {:>8}  {}",
                        prefix, agent.short_id, agent.status, tokens_str, tool_str
                    )?;
                    writeln!(w, "     {}", msg)?;
                } else {
                    writeln!(
                        w,
                        "  {} {:<8}  {:<10}  {:>8}  {}",
                        prefix, agent.short_id, agent.status, tokens_str, tool_str
                    )?;
                }
            } else {
                writeln!(
                    w,
                    "  {} {:<8}  {:<10}  {:>8}  findings={}",
                    prefix, agent.short_id, agent.status, tokens_str, agent.findings
                )?;
            }
        }
    }

    // Source footer
    let source_note = match view.source {
        luft::service::phases::PhasesSource::Meta => "meta",
        luft::service::phases::PhasesSource::EventsFallback => "events fallback",
    };
    if total == 0 {
        writeln!(w, "(no phases started yet)")?;
    }
    writeln!(w, "\nsource: {}", source_note)?;

    Ok(())
}

fn format_elapsed(secs: f64) -> String {
    if secs < 1.0 {
        format!("{:.0}ms", secs * 1000.0)
    } else if secs < 60.0 {
        format!("{:.1}s", secs)
    } else {
        let m = (secs / 60.0) as u64;
        let s = secs % 60.0;
        format!("{}m{:.0}s", m, s)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use luft::core::state::{get_run_store, CheckpointStatus};
    use luft::planner::{MetaPhase, PlanMeta};
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static CWD_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        _dir: TempDir,
        orig_cwd: PathBuf,
    }

    impl TestEnv {
        fn new() -> Self {
            let _lock = CWD_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let dir = TempDir::new().unwrap();
            let orig_cwd = std::env::current_dir().unwrap();
            std::env::set_current_dir(dir.path()).unwrap();
            TestEnv {
                _lock,
                _dir: dir,
                orig_cwd,
            }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.orig_cwd);
        }
    }

    fn seed_run_with_meta(meta: PlanMeta, current_phase: u32) -> String {
        let base_dir = runs_base_dir();
        std::fs::create_dir_all(&base_dir).unwrap();
        let run_uuid = uuid::Uuid::now_v7();
        let dir_name = run_uuid.to_string();
        let store = get_run_store(&dir_name, &base_dir).unwrap();
        let id = uuid::Uuid::now_v7();
        store.init_run_with_meta(id, "test task", serde_json::to_value(&meta).unwrap()).unwrap();
        let mut cp = store.get_checkpoint().unwrap();
        cp.current_phase = current_phase;
        cp.status = CheckpointStatus::Running;
        store.save_checkpoint(&cp).unwrap();
        dir_name
    }

    fn capture(run_dir: String, json: bool) -> (String, anyhow::Result<()>) {
        let mut buf = Vec::new();
        let result = phases_cmd_inner(&mut buf, run_dir, PhasesArgs { json });
        let output = String::from_utf8(buf).expect("not UTF-8");
        (output, result)
    }

    #[test]
    fn run_not_found() {
        let _env = TestEnv::new();
        std::fs::create_dir_all(runs_base_dir()).unwrap();
        let result = phases_cmd("nonexistent".to_string(), PhasesArgs::default());
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("run not found or has no checkpoint"));
    }

    #[test]
    fn meta_full_tree_render() {
        let _env = TestEnv::new();
        let meta = PlanMeta {
            phases: vec![
                MetaPhase { label: "gather".into(), detail: "collect sources".into(), agents: 2, ..Default::default() },
                MetaPhase { label: "report".into(), detail: "summarize".into(), agents: 1, depends_on: vec![1] },
            ],
            reasoning: "two-stage".into(),
        };
        let run_dir = seed_run_with_meta(meta, 1);
        let (out, res) = capture(run_dir, false);
        assert!(res.is_ok(), "error: {:?}", res.err());
        assert!(out.contains("Run "));
        assert!(out.contains("status="));
        assert!(out.contains("current_phase="));
        assert!(out.contains("Task: test task"));
        assert!(out.contains("Phase 1/2  gather"));
        assert!(out.contains("Phase 2/2  report"));
        assert!(out.contains("┊ collect sources"));
        assert!(out.contains("source: meta"));
    }

    #[test]
    fn json_output_structure() {
        let _env = TestEnv::new();
        let meta = PlanMeta {
            phases: vec![MetaPhase { label: "only".into(), detail: "the only one".into(), agents: 1, ..Default::default() }],
            reasoning: "single".into(),
        };
        let run_dir = seed_run_with_meta(meta, 0);
        let (out, res) = capture(run_dir, true);
        assert!(res.is_ok(), "error: {:?}", res.err());
        assert!(out.contains("\"run\""));
        assert!(out.contains("\"phases\""));
        assert!(out.contains("\"meta\""));
    }

    #[test]
    fn no_meta_fallback_events() {
        let _env = TestEnv::new();
        let base_dir = runs_base_dir();
        std::fs::create_dir_all(&base_dir).unwrap();
        let run_uuid = uuid::Uuid::now_v7();
        let dir_name = run_uuid.to_string();
        let store = get_run_store(&dir_name, &base_dir).unwrap();
        let id = uuid::Uuid::now_v7();
        store.init_run(id, "legacy run").unwrap();

        let run_dir_path = base_dir.join(&dir_name);
        let event = serde_json::json!({
            "type": "phase_started",
            "run_id": uuid::Uuid::now_v7().to_string(),
            "phase_id": 1,
            "label": "discovery",
            "planned": 1
        });
        std::fs::write(
            run_dir_path.join("events.jsonl"),
            serde_json::to_string(&event).unwrap(),
        )
        .unwrap();

        let (out, res) = capture(dir_name, false);
        assert!(res.is_ok(), "error: {:?}", res.err());
        assert!(out.contains("discovery"));
        assert!(out.contains("source: events fallback"));
    }

    #[test]
    fn empty_run_no_phases_message() {
        let _env = TestEnv::new();
        let base_dir = runs_base_dir();
        std::fs::create_dir_all(&base_dir).unwrap();
        let run_uuid = uuid::Uuid::now_v7();
        let dir_name = run_uuid.to_string();
        let store = get_run_store(&dir_name, &base_dir).unwrap();
        let id = uuid::Uuid::now_v7();
        store.init_run(id, "empty run").unwrap();

        let (out, res) = capture(dir_name, false);
        assert!(res.is_ok(), "error: {:?}", res.err());
        assert!(out.contains("no phases started yet"));
    }

    #[test]
    fn events_missing_elapsed_question_mark() {
        let _env = TestEnv::new();
        let meta = PlanMeta {
            phases: vec![MetaPhase { label: "p".into(), detail: "d".into(), agents: 1, ..Default::default() }],
            reasoning: String::new(),
        };
        let run_dir = seed_run_with_meta(meta, 1);
        let (out, res) = capture(run_dir, false);
        assert!(res.is_ok(), "error: {:?}", res.err());
        // No events → elapsed shows ?s or a number from checkpoint timestamps
        assert!(out.contains("elapsed="));
    }

    #[test]
    fn pending_phase_shows_no_ok_failed() {
        let _env = TestEnv::new();
        let meta = PlanMeta {
            phases: vec![
                MetaPhase { label: "a".into(), detail: "1".into(), agents: 1, ..Default::default() },
                MetaPhase { label: "b".into(), detail: "2".into(), agents: 1, ..Default::default() },
            ],
            reasoning: String::new(),
        };
        let run_dir = seed_run_with_meta(meta, 0);
        let (out, res) = capture(run_dir, false);
        assert!(res.is_ok());
        // Pending phases should show "pending" without ok/failed
        assert!(out.contains("pending"));
    }

    #[test]
    fn format_elapsed_variants() {
        assert_eq!(format_elapsed(0.5), "500ms");
        assert_eq!(format_elapsed(3.2), "3.2s");
        assert_eq!(format_elapsed(65.0), "1m5s");
    }
}
