//! `clear` subcommand: remove terminal-state runs from `.luft/runs`.
//!
//! Deletes all completed/cancelled/failed runs. Running runs are never
//! touched. Use `--days N` to only clear runs older than N days.

use super::runs_base_dir;
use anyhow::Result;
use std::time::Duration;

pub fn clear_runs_cmd(days: Option<u64>) -> Result<()> {
    let base_dir = runs_base_dir();

    if !base_dir.exists() {
        println!("No runs to clear.");
        return Ok(());
    }

    let older_than = days
        .map(|d| Duration::from_secs(d * 86400))
        .unwrap_or(Duration::ZERO);

    let cleaned = luft::core::gc_runs(&base_dir, older_than)?;

    if cleaned == 0 {
        println!("No runs to clear.");
    } else {
        println!("Cleared {} run(s).", cleaned);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::GLOBAL_CWD_LOCK;
    use super::*;
    use luft::core::state::{get_run_store, CheckpointStatus};
    use std::path::PathBuf;
    use std::sync::MutexGuard;
    use tempfile::TempDir;

    struct TestEnv {
        _lock: MutexGuard<'static, ()>,
        _dir: TempDir,
        orig_cwd: PathBuf,
    }

    impl TestEnv {
        fn new() -> Self {
            let _lock = GLOBAL_CWD_LOCK
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

    fn create_completed_run(task: &str) -> uuid::Uuid {
        let base_dir = runs_base_dir();
        let run_id = uuid::Uuid::now_v7();
        let dir_name = run_id.to_string();
        let store = get_run_store(&dir_name, &base_dir).unwrap();
        store.init_run(run_id, task).unwrap();
        if let Some(mut cp) = store.get_checkpoint() {
            cp.status = CheckpointStatus::Completed;
            cp.updated_at = 1000;
            let _ = store.save_checkpoint(&cp);
        }
        run_id
    }

    fn create_running_run(task: &str) -> uuid::Uuid {
        let base_dir = runs_base_dir();
        let run_id = uuid::Uuid::now_v7();
        let dir_name = run_id.to_string();
        let store = get_run_store(&dir_name, &base_dir).unwrap();
        store.init_run(run_id, task).unwrap();
        run_id
    }

    #[test]
    fn no_runs_dir_prints_not_found() {
        let _env = TestEnv::new();
        assert!(clear_runs_cmd(None).is_ok());
    }

    #[test]
    fn empty_runs_dir() {
        let _env = TestEnv::new();
        std::fs::create_dir_all(runs_base_dir()).unwrap();
        assert!(clear_runs_cmd(None).is_ok());
    }

    #[test]
    fn clears_completed_runs() {
        let _env = TestEnv::new();
        create_completed_run("task 1");
        create_completed_run("task 2");
        assert!(clear_runs_cmd(None).is_ok());
        assert!(runs_base_dir().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn preserves_running_runs() {
        let _env = TestEnv::new();
        create_completed_run("completed");
        create_running_run("still running");
        assert!(clear_runs_cmd(None).is_ok());
        let remaining: Vec<_> = runs_base_dir().read_dir().unwrap().collect();
        assert_eq!(remaining.len(), 1, "running run should survive");
    }

    #[test]
    fn days_filter_skips_recent() {
        let _env = TestEnv::new();
        // Create a completed run with a *recent* timestamp (now).
        let base_dir = runs_base_dir();
        let run_id = uuid::Uuid::now_v7();
        let dir_name = run_id.to_string();
        let store = get_run_store(&dir_name, &base_dir).unwrap();
        store.init_run(run_id, "recent").unwrap();
        if let Some(mut cp) = store.get_checkpoint() {
            cp.status = CheckpointStatus::Completed;
            cp.updated_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let _ = store.save_checkpoint(&cp);
        }
        // With --days 7, the recent run should survive.
        assert!(clear_runs_cmd(Some(7)).is_ok());
        let remaining: Vec<_> = runs_base_dir().read_dir().unwrap().collect();
        assert_eq!(remaining.len(), 1, "recent run should survive --days 7");
    }

    #[test]
    fn days_filter_removes_old() {
        let _env = TestEnv::new();
        create_completed_run("old task");
        assert!(clear_runs_cmd(Some(7)).is_ok());
        assert!(runs_base_dir().read_dir().unwrap().next().is_none());
    }
}
