//! `status` subcommand: show the checkpoint summary of a past run.

use super::runs_base_dir;
use anyhow::Result;
use std::io::Write;

pub fn status_run_cmd(run_dir: String) -> Result<()> {
    status_run_cmd_inner(&mut std::io::stdout(), run_dir)
}

pub(crate) fn status_run_cmd_inner(w: &mut impl Write, run_dir: String) -> Result<()> {
    let base_dir = runs_base_dir();
    let checkpoint = maestro::service::query::get_checkpoint(&run_dir, &base_dir)?
        .ok_or_else(|| anyhow::anyhow!("run not found or has no checkpoint: {}", run_dir))?;

    writeln!(w, "=== Run Status ===")?;
    writeln!(w, "  Run ID:        {}", checkpoint.run_id)?;
    writeln!(w, "  Directory:     {}", run_dir)?;
    writeln!(w, "  Task:          {}", checkpoint.task)?;
    writeln!(w, "  Status:        {:?}", checkpoint.status)?;
    writeln!(w, "  Current phase: {}", checkpoint.current_phase)?;
    writeln!(w, "  Total tokens:  {}", checkpoint.total_tokens)?;
    writeln!(w, "  Created:       {}", checkpoint.created_at)?;
    writeln!(w, "  Updated:       {}", checkpoint.updated_at)?;

    if !checkpoint.completed_phases.is_empty() {
        writeln!(w, "\n  Completed phases:")?;
        for phase in &checkpoint.completed_phases {
            writeln!(
                w,
                "    - Phase {}: {} (ok={}, failed={})",
                phase.phase_id, phase.label, phase.ok, phase.failed
            )?;
        }
    }

    let agent_count = checkpoint.agent_results.len();
    if agent_count > 0 {
        writeln!(w, "\n  Agent results: {} agents", agent_count)?;
    }

    let findings_count = checkpoint.findings.len();
    if findings_count > 0 {
        writeln!(w, "  Findings: {} total", findings_count)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro::core::contract::finding::{Finding, Severity};
    use maestro::core::state::{get_run_store, AgentResultCache, PhaseSummary};
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

    fn create_run(task: &str) -> (String, uuid::Uuid) {
        let base_dir = runs_base_dir();
        std::fs::create_dir_all(&base_dir).unwrap();
        let run_uuid = uuid::Uuid::now_v7();
        let dir_name = run_uuid.to_string();
        let store = get_run_store(&dir_name, &base_dir).unwrap();
        let id = uuid::Uuid::now_v7();
        store.init_run(id, task).unwrap();
        (dir_name, id)
    }

    fn capture_output(run_dir: String) -> (String, anyhow::Result<()>) {
        let mut buf = Vec::new();
        let result = status_run_cmd_inner(&mut buf, run_dir);
        let output = String::from_utf8(buf).expect("not UTF-8");
        (output, result)
    }

    #[test]
    fn run_not_found() {
        let _env = TestEnv::new();
        std::fs::create_dir_all(runs_base_dir()).unwrap();
        let result = status_run_cmd("nonexistent".to_string());
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("run not found or has no checkpoint"));
    }

    #[test]
    fn dir_exists_but_no_checkpoint() {
        let _env = TestEnv::new();
        let base_dir = runs_base_dir();
        std::fs::create_dir_all(&base_dir).unwrap();
        std::fs::create_dir(base_dir.join("empty-run-dir")).unwrap();
        let result = status_run_cmd("empty-run-dir".to_string());
        assert!(result.is_err());
        let msg = format!("{}", result.unwrap_err());
        assert!(msg.contains("run not found or has no checkpoint"));
    }

    #[test]
    fn empty_checkpoint() {
        let _env = TestEnv::new();
        let (run_dir, id) = create_run("test task");
        let (output, result) = capture_output(run_dir);
        assert!(result.is_ok());
        assert!(output.contains("=== Run Status ==="));
        assert!(output.contains(&id.to_string()));
        assert!(output.contains("test task"));
        assert!(output.contains("Running"));
    }

    #[test]
    fn with_completed_phases() {
        let _env = TestEnv::new();
        let (run_dir, _) = create_run("test task");
        let base_dir = runs_base_dir();
        let store = get_run_store(&run_dir, &base_dir).unwrap();
        let mut cp = store.get_checkpoint().unwrap();
        cp.completed_phases.push(PhaseSummary {
            phase_id: 1,
            label: "Planning".to_string(),
            planned: 3,
            ok: 2,
            failed: 1,
            description: None,
            role: None,
        });
        store.save_checkpoint(&cp).unwrap();
        let (output, result) = capture_output(run_dir);
        assert!(result.is_ok());
        assert!(output.contains("Planning"));
        assert!(output.contains("ok=2"));
        assert!(output.contains("failed=1"));
    }

    #[test]
    fn with_agent_results() {
        let _env = TestEnv::new();
        let (run_dir, _) = create_run("test task");
        let base_dir = runs_base_dir();
        let store = get_run_store(&run_dir, &base_dir).unwrap();
        let mut cp = store.get_checkpoint().unwrap();
        cp.agent_results.insert(
            uuid::Uuid::now_v7(),
            AgentResultCache {
                agent_id: uuid::Uuid::now_v7(),
                phase_id: 1,
                status: "completed".to_string(),
                output: serde_json::Value::Null,
                findings: vec![],
                tokens: 100,
                completed_at: 1234567890,
                cache_key_hash: None,
            },
        );
        store.save_checkpoint(&cp).unwrap();
        let (output, result) = capture_output(run_dir);
        assert!(result.is_ok());
        assert!(output.contains("Agent results: 1 agents"));
    }

    #[test]
    fn with_findings() {
        let _env = TestEnv::new();
        let (run_dir, _) = create_run("test task");
        let base_dir = runs_base_dir();
        let store = get_run_store(&run_dir, &base_dir).unwrap();
        let mut cp = store.get_checkpoint().unwrap();
        cp.findings.push(Finding {
            kind: "test".to_string(),
            severity: Severity::Info,
            title: "Test finding".to_string(),
            detail: "A test finding".to_string(),
            location: None,
            evidence: vec![],
            data: serde_json::Value::Null,
        });
        store.save_checkpoint(&cp).unwrap();
        let (output, result) = capture_output(run_dir);
        assert!(result.is_ok());
        assert!(output.contains("Findings: 1 total"));
    }

    #[test]
    fn run_dir_is_file_io_error() {
        let _env = TestEnv::new();
        let base_dir = runs_base_dir();
        std::fs::create_dir_all(&base_dir).unwrap();
        let run_dir_name = "path-is-file-not-dir";
        std::fs::write(
            base_dir.join(run_dir_name),
            "i am a file, not a directory",
        )
        .unwrap();
        let result = status_run_cmd(run_dir_name.to_string());
        assert!(result.is_err(), "expected Err when run_dir is a file");
        let msg = format!("{}", result.unwrap_err());
        assert!(
            msg.contains("File exists")
                || msg.contains("Not a directory")
                || msg.contains("would be a file")
                || msg.contains("Is a directory")
                || msg.contains("is a directory"),
            "unexpected error message: {msg}"
        );
    }

    #[test]
    fn with_all_data() {
        let _env = TestEnv::new();
        let (run_dir, _) = create_run("test task");
        let base_dir = runs_base_dir();
        let store = get_run_store(&run_dir, &base_dir).unwrap();
        let mut cp = store.get_checkpoint().unwrap();
        cp.completed_phases.push(PhaseSummary {
            phase_id: 1,
            label: "Research".to_string(),
            planned: 5,
            ok: 5,
            failed: 0,
            description: None,
            role: None,
        });
        cp.completed_phases.push(PhaseSummary {
            phase_id: 2,
            label: "Implement".to_string(),
            planned: 10,
            ok: 8,
            failed: 2,
            description: None,
            role: None,
        });
        cp.agent_results.insert(
            uuid::Uuid::now_v7(),
            AgentResultCache {
                agent_id: uuid::Uuid::now_v7(),
                phase_id: 1,
                status: "completed".to_string(),
                output: serde_json::json!({"result": "ok"}),
                findings: vec![],
                tokens: 500,
                completed_at: 1234567890,
                cache_key_hash: None,
            },
        );
        cp.findings.push(Finding {
            kind: "bug".to_string(),
            severity: Severity::High,
            title: "Null pointer".to_string(),
            detail: "Potential null dereference".to_string(),
            location: None,
            evidence: vec!["line 42".to_string()],
            data: serde_json::Value::Null,
        });
        store.save_checkpoint(&cp).unwrap();
        let (output, result) = capture_output(run_dir);
        assert!(result.is_ok());
        assert!(output.contains("Research"));
        assert!(output.contains("Implement"));
        assert!(output.contains("ok=5"));
        assert!(output.contains("failed=2"));
        assert!(output.contains("Agent results: 1 agents"));
        assert!(output.contains("Findings: 1 total"));
    }
}
