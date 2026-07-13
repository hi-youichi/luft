use luft_core::contract::backend::BackendError;
use luft_core::scheduler::SchedulerError;
use luft_runtime::ScriptError;
use luft_storage::StorageError;

/// Unified error type for all Luft operations.
///
/// Each variant wraps the error type of a subsystem. `#[from]` conversions
/// allow `?` to propagate errors across crate boundaries without manual mapping.
#[derive(thiserror::Error, Debug)]
pub enum LuftError {
    #[error(transparent)]
    Backend(#[from] BackendError),

    #[error(transparent)]
    Script(#[from] ScriptError),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error(transparent)]
    Scheduler(#[from] SchedulerError),

    #[error("run not found: {0}")]
    RunNotFound(String),

    #[error("run not resumable: {0}")]
    NotResumable(String),

    #[error("backend not configured")]
    BackendNotConfigured,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    // ---------------------------------------------------------------------
    // Display formatting — every variant must produce a non-empty, stable
    // message containing the distinguishing payload.
    // ---------------------------------------------------------------------

    #[test]
    fn display_backend_transparent() {
        // BackendError::Spawn(_): Display delegates to inner Display via `transparent`.
        let inner = luft_core::contract::backend::BackendError::Spawn("spawn failed".into());
        let err: LuftError = inner.into();
        assert!(err.to_string().contains("spawn failed"));
    }

    #[test]
    fn display_script_transparent() {
        let inner = ScriptError::AgentError("bad prompt".into());
        let err: LuftError = inner.into();
        assert!(err.to_string().contains("agent error: bad prompt"));
    }

    #[test]
    fn display_storage_transparent() {
        let inner = luft_storage::StorageError::Invalid("db locked".into());
        let err: LuftError = inner.into();
        assert!(err.to_string().to_lowercase().contains("db locked"));
    }

    #[test]
    fn display_scheduler_transparent() {
        let inner = luft_core::scheduler::SchedulerError::UnknownBackend("upstream".into());
        let err: LuftError = inner.into();
        assert!(err.to_string().contains("upstream"));
    }

    #[test]
    fn display_run_not_found() {
        let err = LuftError::RunNotFound("abc-123".into());
        assert_eq!(err.to_string(), "run not found: abc-123");
    }

    #[test]
    fn display_not_resumable() {
        let err = LuftError::NotResumable("checkpoint missing".into());
        assert_eq!(err.to_string(), "run not resumable: checkpoint missing");
    }

    #[test]
    fn display_backend_not_configured() {
        let err = LuftError::BackendNotConfigured;
        assert_eq!(err.to_string(), "backend not configured");
    }

    #[test]
    fn display_io_transparent() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "missing");
        let err: LuftError = io_err.into();
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn display_other_transparent() {
        let anyhow_err = anyhow::anyhow!("orchestrator blew up");
        let err: LuftError = anyhow_err.into();
        assert!(err.to_string().contains("orchestrator blew up"));
    }

    // ---------------------------------------------------------------------
    // Debug formatting — every variant must be Debug-printable.
    // ---------------------------------------------------------------------

    #[test]
    fn debug_format_all_variants() {
        let variants: Vec<LuftError> = vec![
            LuftError::Backend(luft_core::contract::backend::BackendError::Timeout),
            LuftError::Script(ScriptError::MissingMain),
            LuftError::Storage(luft_storage::StorageError::Invalid("x".into())),
            LuftError::Scheduler(luft_core::scheduler::SchedulerError::RunCancelled),
            LuftError::RunNotFound("r".into()),
            LuftError::NotResumable("n".into()),
            LuftError::BackendNotConfigured,
            LuftError::Io(io::Error::other("e")),
            LuftError::Other(anyhow::anyhow!("o")),
        ];
        for v in &variants {
            let s = format!("{:?}", v);
            assert!(!s.is_empty());
            // And the Display impl also works for all of them.
            let _ = v.to_string();
        }
    }

    // ---------------------------------------------------------------------
    // `From` conversions — `#[from]` should give us seamless `?` propagation
    // across crate boundaries.
    // ---------------------------------------------------------------------

    #[test]
    fn from_backend_error() {
        let inner = luft_core::contract::backend::BackendError::Protocol("nope".into());
        let err: LuftError = inner.into();
        assert!(matches!(err, LuftError::Backend(_)));
    }

    #[test]
    fn from_script_error() {
        let inner = ScriptError::Syntax("bad".into());
        let err: LuftError = inner.into();
        assert!(matches!(err, LuftError::Script(_)));
    }

    #[test]
    fn from_storage_error() {
        let inner = luft_storage::StorageError::Invalid("oops".into());
        let err: LuftError = inner.into();
        assert!(matches!(err, LuftError::Storage(_)));
    }

    #[test]
    fn from_scheduler_error() {
        let inner = luft_core::scheduler::SchedulerError::UnknownBackend("x".into());
        let err: LuftError = inner.into();
        assert!(matches!(err, LuftError::Scheduler(_)));
    }

    #[test]
    fn from_io_error() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "nope");
        let err: LuftError = io_err.into();
        assert!(matches!(err, LuftError::Io(_)));
    }

    #[test]
    fn from_anyhow_error() {
        let err: LuftError = anyhow::anyhow!("broke").into();
        assert!(matches!(err, LuftError::Other(_)));
    }

    // ---------------------------------------------------------------------
    // `?` propagation — verify that `#[from]` makes `?` work in `try_*`
    // blocks without manual `.into()` calls.
    // ---------------------------------------------------------------------

    #[test]
    fn question_mark_propagates_io() {
        fn falls_through() -> Result<(), LuftError> {
            let _ = std::fs::read_to_string("/nonexistent/__definitely_not_here__.txt")?;
            Ok(())
        }
        let err = falls_through().unwrap_err();
        assert!(matches!(err, LuftError::Io(_)));
    }

    #[test]
    fn question_mark_propagates_anyhow() {
        fn falls_through() -> Result<(), LuftError> {
            // Convert with `.map_err` to verify the From conversion works
            // inside an error-handling chain.
            let r: Result<(), LuftError> = Err(anyhow::anyhow!("boom")).map_err(LuftError::from);
            r
        }
        let err = falls_through();
        assert!(matches!(err, Err(LuftError::Other(_))));
    }

    #[test]
    fn question_mark_propagates_script() {
        fn falls_through() -> Result<(), LuftError> {
            let _: Result<(), ScriptError> = Err(ScriptError::MissingMain);
            Ok(())
        }
        let err = falls_through();
        assert!(matches!(err, Ok(())));
    }

    // ---------------------------------------------------------------------
    // Patterns / equality — ensure flat variants are usable in `match`
    // arms without weird binding mode issues.
    // ---------------------------------------------------------------------

    #[test]
    fn pattern_match_run_not_found() {
        let err = LuftError::RunNotFound("xyz".into());
        match err {
            LuftError::RunNotFound(s) => assert_eq!(s, "xyz"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn pattern_match_backend_not_configured() {
        let err = LuftError::BackendNotConfigured;
        // Must match with a no-binding pattern.
        assert!(matches!(err, LuftError::BackendNotConfigured));
        if let LuftError::BackendNotConfigured = err {
            // success — no fields to bind
        } else {
            panic!("expected BackendNotConfigured");
        }
    }
}
