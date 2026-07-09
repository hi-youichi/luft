//! Binary command handlers — one module per CLI subcommand. `main` parses
//! args and routes each subcommand to the matching handler here.

pub mod artifact_writer;
pub mod backend;
pub mod clear;
pub mod event_log;
pub mod generate;
pub mod list;
pub mod logs;
pub mod lua_validate;
pub mod mcp_server;
pub mod mock;
pub mod phase_renderer;
pub mod phases;
pub mod run;
pub mod save;
pub mod status;
pub mod workflows;

/// Runs are stored in `.maestro/runs` relative to the current working directory.
pub(crate) fn runs_base_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(".").join(".maestro").join("runs")
}

/// Shared global CWD lock for tests that change the working directory.
/// Individual test modules (list, status, logs) MUST use this instead of
/// their own local mutex to prevent cross-module CWD races.
#[cfg(test)]
pub(crate) static GLOBAL_CWD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::mpsc;

    // -----------------------------------------------------------------
    // runs_base_dir() — happy path, edge cases, boundaries
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::serial]
    fn runs_base_dir_last_component_is_runs() {
        let p = runs_base_dir();
        assert_eq!(
            p.file_name().and_then(|s| s.to_str()),
            Some("runs"),
            "terminal component must be `runs`, got {:?}",
            p
        );
    }

    #[test]
    #[serial_test::serial]
    fn runs_base_dir_immediate_parent_is_dot_maestro() {
        let p = runs_base_dir();
        let parent = p
            .parent()
            .expect("runs_base_dir() must have a parent component");
        assert_eq!(
            parent.file_name().and_then(|s| s.to_str()),
            Some(".maestro"),
            "parent must be `.maestro`, got {:?}",
            parent
        );
    }

    #[test]
    #[serial_test::serial]
    fn runs_base_dir_components_chain_through_dot_then_dot_maestro() {
        let p = runs_base_dir();
        let comps: Vec<String> = p
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            comps,
            vec![".".to_string(), ".maestro".to_string(), "runs".to_string(),],
            "unexpected path component chain: {:?}",
            comps
        );
    }

    #[test]
    #[serial_test::serial]
    fn runs_base_dir_is_relative_and_not_rooted() {
        let p = runs_base_dir();
        assert!(p.is_relative(), "expected relative path, got {:?}", p);
        assert!(!p.has_root(), "expected no root component, got {:?}", p);
        assert_eq!(p, Path::new(".").join(".maestro").join("runs"));
    }

    #[test]
    #[serial_test::serial]
    fn runs_base_dir_equals_manually_constructed_path() {
        let expected: PathBuf = PathBuf::from(".").join(".maestro").join("runs");
        assert_eq!(runs_base_dir(), expected);
    }

    #[test]
    #[serial_test::serial]
    fn runs_base_dir_is_independent_of_cwd() {
        let _lock = GLOBAL_CWD_LOCK.lock().unwrap();
        let scratch1 = tempfile::tempdir().unwrap();
        let scratch2 = tempfile::tempdir().unwrap();
        let original = std::env::current_dir().unwrap();

        std::env::set_current_dir(scratch1.path()).unwrap();
        let p1 = runs_base_dir();
        std::env::set_current_dir(scratch2.path()).unwrap();
        let p2 = runs_base_dir();
        std::env::set_current_dir(&original).unwrap();
        let p3 = runs_base_dir();

        assert_eq!(p1, p2);
        assert_eq!(p2, p3);
    }

    #[test]
    #[serial_test::serial]
    fn runs_base_dir_each_call_returns_a_fresh_pathbuf() {
        let mut first = runs_base_dir();
        let second = runs_base_dir();
        first.push("child-segment");
        assert_ne!(first, second);
    }

    #[test]
    #[serial_test::serial]
    fn runs_base_dir_joined_child_path_stays_under_base() {
        let base = runs_base_dir();
        let child = base.join("a-run-uuid");
        assert!(
            child.starts_with(&base),
            "joined path {:?} should still begin with base {:?}",
            child,
            base
        );
    }

    #[test]
    #[serial_test::serial]
    fn runs_base_dir_joined_children_are_distinct() {
        let a = runs_base_dir().join("run-a");
        let b = runs_base_dir().join("run-b");
        assert_ne!(a, b);
        assert_eq!(
            a.parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str()),
            Some("runs")
        );
    }

    // -----------------------------------------------------------------
    // GLOBAL_CWD_LOCK — happy path, edge cases, boundaries
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::serial]
    fn global_cwd_lock_acquires_and_releases() {
        {
            let _guard = GLOBAL_CWD_LOCK.lock().unwrap();
        }
        let _guard = GLOBAL_CWD_LOCK.lock().unwrap();
    }

    #[test]
    #[serial_test::serial]
    fn global_cwd_lock_blocks_concurrent_acquire_via_try_lock() {
        let _outer = GLOBAL_CWD_LOCK.lock().unwrap();
        let result = GLOBAL_CWD_LOCK.try_lock();
        assert!(
            result.is_err(),
            "expected `try_lock` to fail while a guard is held"
        );
    }

    #[test]
    #[serial_test::serial]
    fn global_cwd_lock_releases_for_next_caller() {
        let g1 = GLOBAL_CWD_LOCK.lock().unwrap();
        drop(g1);
        assert!(GLOBAL_CWD_LOCK.try_lock().is_ok());
    }

    #[test]
    #[serial_test::serial]
    fn global_cwd_lock_is_static_lifetime() {
        // The static must hand out a guard with `'static` lifetime so it can
        // be embedded in long-lived structs (the `TestEnv` pattern in
        // `list.rs`, `status.rs`, `logs.rs`, `clear.rs`).
        let guard: std::sync::MutexGuard<'static, ()> = GLOBAL_CWD_LOCK.lock().unwrap();
        // Use the guard; if the lock returned a shorter lifetime the
        // annotation on `guard` would fail to compile.
        let _used = &*guard;
    }

    #[test]
    #[serial_test::serial]
    fn global_cwd_lock_is_shared_across_threads() {
        // We hold the main-thread guard for the whole test so other tests in
        // the same binary running in parallel cannot briefly steal the lock
        // (the lock is `pub(crate) static` — global to the test binary).
        let _main_guard = GLOBAL_CWD_LOCK.lock().unwrap();

        let (tx_acquired, rx_acquired) = mpsc::channel::<()>();
        let (tx_release_ack, rx_release_ack) = mpsc::channel::<()>();

        let worker = std::thread::spawn(move || {
            // Block on the contended lock until the main guard is dropped.
            // This proves the lock CAN be acquired by another thread when the
            // main thread holds it.
            let _guard = GLOBAL_CWD_LOCK.lock().unwrap();
            tx_acquired.send(()).unwrap();
            rx_release_ack.recv().unwrap();
            drop(_guard);
        });

        // While the main thread holds `_main_guard`, the worker thread's
        // `lock()` call will block — we verify that by waiting for the
        // `acquired` channel which can only fire after the worker gets the
        // guard, which can only happen once `_main_guard` is dropped.
        //
        // Conversely, if the lock were per-thread instead of global, the
        // worker would acquire instantly even with `_main_guard` alive — so
        // this assertion catches a regression where the lock is no longer
        // shared.
        drop(_main_guard);
        rx_acquired
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("worker thread must acquire after main guard is released");
        assert!(
            GLOBAL_CWD_LOCK.try_lock().is_err(),
            "main thread must NOT acquire while worker holds the lock"
        );

        tx_release_ack.send(()).unwrap();
        worker.join().unwrap();

        assert!(
            GLOBAL_CWD_LOCK.try_lock().is_ok(),
            "main thread MUST acquire after worker releases"
        );
    }

    // -----------------------------------------------------------------
    // Submodule declaration check (compile-time).
    // -----------------------------------------------------------------

    #[test]
    #[serial_test::serial]
    fn all_fifteen_subcommand_modules_are_declared() {
        #[allow(unused_imports)]
        {
            use super::artifact_writer as _aw;
            use super::backend as _bk;
            use super::clear as _cl;
            use super::event_log as _el;
            use super::generate as _gn;
            use super::list as _ls;
            use super::logs as _lg;
            use super::lua_validate as _lv;
            use super::mcp_server as _ms;
            use super::mock as _mk;
            use super::phase_renderer as _pr;
            use super::run as _rn;
            use super::save as _sv;
            use super::status as _st;
            use super::workflows as _wf;
        }
    }
}
