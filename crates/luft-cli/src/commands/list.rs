//! `list` subcommand: list past runs (most recent first).

use super::runs_base_dir;
use anyhow::Result;

/// Default cap when the caller doesn't pass an explicit `--limit`.
pub const DEFAULT_LIMIT: usize = 20;

/// Build the user-facing listing as a single `String`.
///
/// Pure aside from the underlying service call so tests can assert on the
/// rendered output without capturing stdout.
pub fn format_runs(limit: Option<usize>) -> Result<String> {
    let base_dir = runs_base_dir();
    let runs = luft::service::query::list_runs(&base_dir)?;
    if runs.is_empty() {
        return Ok("No runs found.\n".to_string());
    }

    let total = runs.len();
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    let shown: Vec<_> = runs.into_iter().take(limit).collect();

    let mut out = String::new();
    out.push_str(&format!(
        "Past runs ({} total, showing {}):\n",
        total,
        shown.len()
    ));
    for run in shown {
        out.push_str(&format!("  {}  [{}]\n", run.run_dir, run.status));
    }
    Ok(out)
}

/// CLI entry: print the listing produced by `format_runs`.
pub fn list_runs_cmd(limit: Option<usize>) -> Result<()> {
    print!("{}", format_runs(limit)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Drops the temp-dir *after* restoring CWD so the relative
    /// `.luft/runs` path stays valid for the duration of the test.
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

    /// Helper: create a run in the current CWD's `.luft/runs/` directory.
    fn create_run(task: &str) -> uuid::Uuid {
        let base_dir = runs_base_dir();
        let run_id = uuid::Uuid::now_v7();
        let dir_name = run_id.to_string();
        let store = luft::core::state::get_run_store(&dir_name, &base_dir).unwrap();
        store.init_run(run_id, task).unwrap();
        run_id
    }

    /// Test helper: create N runs in one call.
    fn create_runs(n: usize) {
        for i in 0..n {
            create_run(&format!("task {}", i));
        }
    }

    /// Count indented run lines (header lines start with "Past runs").
    fn run_line_count(body: &str) -> usize {
        body.lines().filter(|l| l.starts_with("  ")).count()
    }

    // -----------------------------------------------------------------
    //  Tests
    // -----------------------------------------------------------------

    #[test]
    fn empty_runs_prints_not_found() {
        let _env = TestEnv::new();
        std::fs::create_dir_all(runs_base_dir()).unwrap();
        let body = format_runs(None).unwrap();
        assert!(
            body.contains("No runs found."),
            "expected empty-state message, got: {body:?}"
        );
        assert_eq!(run_line_count(&body), 0);
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
        let body = format_runs(None).unwrap();
        assert!(
            body.contains("Past runs (1 total, showing 1):"),
            "got: {body:?}"
        );
        assert_eq!(run_line_count(&body), 1);
    }

    #[test]
    fn multiple_runs_all_shown_without_limit() {
        let _env = TestEnv::new();
        create_runs(5);
        let body = format_runs(None).unwrap();
        assert!(
            body.contains("Past runs (5 total, showing 5):"),
            "got: {body:?}"
        );
        assert_eq!(run_line_count(&body), 5);
    }

    #[test]
    fn limit_fewer_than_total() {
        let _env = TestEnv::new();
        create_runs(10);
        let body = format_runs(Some(3)).unwrap();
        assert!(
            body.contains("Past runs (10 total, showing 3):"),
            "got: {body:?}"
        );
        assert_eq!(run_line_count(&body), 3);
    }

    #[test]
    fn limit_equal_to_total() {
        let _env = TestEnv::new();
        create_runs(3);
        let body = format_runs(Some(3)).unwrap();
        assert!(
            body.contains("Past runs (3 total, showing 3):"),
            "got: {body:?}"
        );
        assert_eq!(run_line_count(&body), 3);
    }

    #[test]
    fn default_limit_caps_at_twenty() {
        let _env = TestEnv::new();
        create_runs(25);
        let body = format_runs(None).unwrap();
        assert!(
            body.contains(&format!("Past runs (25 total, showing {}):", DEFAULT_LIMIT)),
            "got: {body:?}"
        );
        assert_eq!(run_line_count(&body), DEFAULT_LIMIT);
    }

    #[test]
    fn limit_zero() {
        let _env = TestEnv::new();
        create_runs(5);
        let body = format_runs(Some(0)).unwrap();
        assert!(
            body.contains("Past runs (5 total, showing 0):"),
            "got: {body:?}"
        );
        assert_eq!(run_line_count(&body), 0);
    }

    #[test]
    fn io_error_propagates() {
        // When the runs base path is a file instead of a directory,
        // `read_dir` inside `list_runs` returns an I/O error.
        let _env = TestEnv::new();
        let base_dir = runs_base_dir();
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
        let body = format_runs(None).unwrap();
        assert!(
            body.contains("Past runs (1 total, showing 1):"),
            "got: {body:?}"
        );
        assert_eq!(run_line_count(&body), 1);
    }
}
