//! `logs` subcommand: dump the (most recent N) event log of a past run.

use super::runs_base_dir;
use anyhow::Result;

pub fn logs_run_cmd(run_dir: String, limit: Option<usize>) -> Result<()> {
    let base_dir = runs_base_dir();
    if !base_dir.join(&run_dir).exists() {
        anyhow::bail!("run not found: {}", run_dir);
    }
    let events = maestro::service::query::get_events(&run_dir, &base_dir)?;
    if events.is_empty() {
        println!("No events for run {}", run_dir);
        return Ok(());
    }

    let limit = limit.unwrap_or(100);
    let events: Vec<_> = events.into_iter().rev().take(limit).rev().collect();

    for event in events {
        let json = serde_json::to_string(&event)?;
        println!("{}", json);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
            let _lock = CWD_LOCK.lock().unwrap();
            let dir = TempDir::new().unwrap();
            let orig_cwd = std::env::current_dir().unwrap();
            std::env::set_current_dir(dir.path()).unwrap();
            TestEnv { _lock, _dir: dir, orig_cwd }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.orig_cwd);
        }
    }

    fn rid() -> uuid::Uuid {
        uuid::Uuid::now_v7()
    }

    fn create_run_with_events(events: &[maestro::core::contract::event::AgentEvent]) -> String {
        let base_dir = runs_base_dir();
        let run_id = rid();
        let dir_name = run_id.to_string();
        let store = maestro::core::state::get_run_store(&dir_name, &base_dir).unwrap();
        store.init_run(run_id, "test task").unwrap();
        for event in events {
            store.append_event(event).unwrap();
        }
        dir_name
    }

    #[test]
    fn run_not_found() {
        let _env = TestEnv::new();
        let err = logs_run_cmd("nonexistent".into(), None).unwrap_err();
        assert!(err.to_string().contains("run not found: nonexistent"), "got: {err}");
    }

    #[test]
    fn base_dir_does_not_exist() {
        let _env = TestEnv::new();
        let err = logs_run_cmd("some-run".into(), None).unwrap_err();
        assert!(err.to_string().contains("run not found: some-run"), "got: {err}");
    }

    #[test]
    fn no_events() {
        let _env = TestEnv::new();
        let run_dir = create_run_with_events(&[]);
        assert!(logs_run_cmd(run_dir, None).is_ok());
    }

    #[test]
    fn with_events_default_limit() {
        let _env = TestEnv::new();
        let run_id = rid();
        let events = [
            maestro::core::contract::event::AgentEvent::RunStarted {
                run_id,
                task: "my task".into(),
                ts: chrono::Utc::now(),
            },
            maestro::core::contract::event::AgentEvent::Log {
                run_id,
                agent_id: None,
                level: maestro::core::contract::event::LogLevel::Info,
                msg: "hello".into(),
            },
        ];
        let run_dir = create_run_with_events(&events);
        assert!(logs_run_cmd(run_dir, None).is_ok());
    }

    #[test]
    fn with_events_custom_limit() {
        let _env = TestEnv::new();
        let run_id = rid();
        let events: Vec<_> = (0..10)
            .map(|i| maestro::core::contract::event::AgentEvent::Log {
                run_id,
                agent_id: None,
                level: maestro::core::contract::event::LogLevel::Info,
                msg: format!("msg {}", i),
            })
            .collect();
        let run_dir = create_run_with_events(&events);
        assert!(logs_run_cmd(run_dir, Some(3)).is_ok());
    }

    #[test]
    fn limit_exceeds_event_count() {
        let _env = TestEnv::new();
        let run_id = rid();
        let events: Vec<_> = (0..3)
            .map(|i| maestro::core::contract::event::AgentEvent::Log {
                run_id,
                agent_id: None,
                level: maestro::core::contract::event::LogLevel::Info,
                msg: format!("msg {}", i),
            })
            .collect();
        let run_dir = create_run_with_events(&events);
        assert!(logs_run_cmd(run_dir, Some(100)).is_ok());
    }

    #[test]
    fn limit_zero_shows_nothing() {
        let _env = TestEnv::new();
        let run_id = rid();
        let events: Vec<_> = (0..5)
            .map(|i| maestro::core::contract::event::AgentEvent::Log {
                run_id,
                agent_id: None,
                level: maestro::core::contract::event::LogLevel::Info,
                msg: format!("msg {}", i),
            })
            .collect();
        let run_dir = create_run_with_events(&events);
        assert!(logs_run_cmd(run_dir, Some(0)).is_ok());
    }

    #[test]
    fn multiple_event_types() {
        let _env = TestEnv::new();
        let run_id = rid();
        use maestro::core::contract::event::*;
        use maestro::core::contract::ids::TokenUsage;
        use maestro::core::contract::backend::AgentStatus;
        let events = vec![
            AgentEvent::RunStarted { run_id, task: "t".into(), ts: chrono::Utc::now() },
            AgentEvent::PhaseStarted { run_id, phase_id: 0, label: "phase 0".into(), planned: 2, parent_span_id: None, description: None, role: None },
            AgentEvent::AgentStarted {
                run_id,
                phase_id: 0,
                agent_id: rid(),
                prompt_preview: "do stuff".into(),
                model: Some("gpt-4".into()),
            },
            AgentEvent::AgentProgress {
                run_id,
                agent_id: rid(),
                delta: ProgressDelta::Message { text: "working...".into() },
            },
            AgentEvent::AgentDone {
                run_id,
                agent_id: rid(),
                status: AgentStatus::Ok,
                tokens: TokenUsage { input: 10, output: 5, cache_read: 0, cache_write: 0 },
                elapsed_ms: 500,
            },
            AgentEvent::PhaseDone { run_id, phase_id: 0, ok: 1, failed: 0 },
            AgentEvent::RunDone {
                run_id,
                status: RunStatus::Completed,
                total_tokens: TokenUsage { input: 10, output: 5, cache_read: 0, cache_write: 0 },
                report: serde_json::json!({"result": "done"}),
            },
            AgentEvent::Log {
                run_id,
                agent_id: None,
                level: LogLevel::Warn,
                msg: "watch out".into(),
            },
        ];
        let run_dir = create_run_with_events(&events);
        assert!(logs_run_cmd(run_dir, None).is_ok());
    }
}
