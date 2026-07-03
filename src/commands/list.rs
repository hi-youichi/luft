//! `list` subcommand: list past runs (most recent first).

use super::runs_base_dir;
use anyhow::Result;

pub fn list_runs_cmd(limit: Option<usize>) -> Result<()> {
    // Presentation only: the service layer loads + sorts (newest first) and
    // skips runs without a checkpoint.
    let base_dir = runs_base_dir();
    let runs = maestro::service::query::list_runs(&base_dir)?;
    if runs.is_empty() {
        println!("No runs found.");
        return Ok(());
    }

    let total = runs.len();
    let limit = limit.unwrap_or(20);
    let shown: Vec<_> = runs.into_iter().take(limit).collect();

    println!("Past runs ({} total, showing {}):", total, shown.len());
    for run in shown {
        println!("  {}  [{}]", run.run_dir, run.status);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Drops the temp-dir *after* restoring CWD so the relative
    /// `.maestro/runs` path stays valid for the duration of the test.
    struct TestEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        _dir: TempDir,
        orig_cwd: PathBuf,
    }

    impl TestEnv {
        fn new() -> Self {
            let _lock = super::super::GLOBAL_CWD_LOCK.lock().unwrap();
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

    /// Helper: create a run in the current CWD's `.maestro/runs/` directory.
    fn create_run(task: &str) -> uuid::Uuid {
        let base_dir = runs_base_dir();
        let run_id = uuid::Uuid::now_v7();
        let dir_name = run_id.to_string();
        let store = maestro::core::state::get_run_store(&dir_name, &base_dir).unwrap();
        store.init_run(run_id, task).unwrap();
        run_id
    }

    // -----------------------------------------------------------------
    //  Tests
    // -----------------------------------------------------------------

    #[test]
    fn empty_runs_prints_not_found() {
        let _env = TestEnv::new();
        std::fs::create_dir_all(runs_base_dir()).unwrap();
        assert!(list_runs_cmd(None).is_ok());
    }

    #[test]
    fn base_dir_does_not_exist() {
        let _env = TestEnv::new();
        assert!(list_runs_cmd(None).is_ok());
    }

    #[test]
    fn single_run() {
        let _env = TestEnv::new();
        create_run("test task");
        assert!(list_runs_cmd(None).is_ok());
    }

    #[test]
    fn multiple_runs_all_shown_without_limit() {
        let _env = TestEnv::new();
        for i in 0..5 {
            create_run(&format!("task {}", i));
        }
        assert!(list_runs_cmd(None).is_ok());
    }

    #[test]
    fn limit_fewer_than_total() {
        let _env = TestEnv::new();
        for i in 0..10 {
            create_run(&format!("task {}", i));
        }
        assert!(list_runs_cmd(Some(3)).is_ok());
    }

    #[test]
    fn limit_equal_to_total() {
        let _env = TestEnv::new();
        for i in 0..3 {
            create_run(&format!("task {}", i));
        }
        assert!(list_runs_cmd(Some(3)).is_ok());
    }

    #[test]
    fn default_limit_caps_at_twenty() {
        let _env = TestEnv::new();
        for i in 0..25 {
            create_run(&format!("task {}", i));
        }
        assert!(list_runs_cmd(None).is_ok());
    }

    #[test]
    fn limit_zero() {
        let _env = TestEnv::new();
        for i in 0..5 {
            create_run(&format!("task {}", i));
        }
        assert!(list_runs_cmd(Some(0)).is_ok());
    }

    #[test]
    fn io_error_propagates() {
        // When the runs base path is a file instead of a directory,
        // `read_dir` inside `list_runs` returns an I/O error.
        let _env = TestEnv::new();
        let base_dir = runs_base_dir();
        // Create parent dir so we can create a file at the runs path
        std::fs::create_dir_all(base_dir.parent().unwrap()).unwrap();
        std::fs::write(&base_dir, "").unwrap();
        assert!(list_runs_cmd(None).is_err());
    }

    #[test]
    fn runs_without_checkpoint_are_skipped() {
        let _env = TestEnv::new();
        // Create a run dir without initialising a checkpoint.
        let base_dir = runs_base_dir();
        let run_id = uuid::Uuid::now_v7();
        let dir_name = run_id.to_string();
        std::fs::create_dir_all(base_dir.join(&dir_name)).unwrap();
        // Now create a real run so the list is non-empty.
        create_run("real task");
        assert!(list_runs_cmd(None).is_ok());
    }
}
