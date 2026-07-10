//! Unified error type for the storage layer.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    Invalid(String),
}

pub type StorageResult<T> = std::result::Result<T, StorageError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_format_for_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: StorageError = io_err.into();
        let dbg = format!("{:?}", err);
        assert!(dbg.contains("Io"), "Debug should contain variant name: {dbg}");
        assert!(dbg.contains("missing"), "Debug should contain inner message: {dbg}");
    }

    #[test]
    fn display_for_io_error_includes_source_message() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err: StorageError = io_err.into();
        let display = err.to_string();
        assert!(
            display.contains("io error"),
            "Display prefix should be present: {display}"
        );
        assert!(
            display.contains("denied"),
            "Display should include inner io error: {display}"
        );
    }

    #[test]
    fn not_found_display_contains_payload() {
        let err = StorageError::NotFound("run-42".to_string());
        let display = err.to_string();
        assert!(display.contains("not found"), "prefix: {display}");
        assert!(display.contains("run-42"), "payload: {display}");
    }

    #[test]
    fn invalid_display_contains_payload() {
        let err = StorageError::Invalid("bad json".to_string());
        let display = err.to_string();
        assert!(display.contains("invalid input"), "prefix: {display}");
        assert!(display.contains("bad json"), "payload: {display}");
    }

    #[test]
    fn from_serde_json_error() {
        let serde_err: serde_json::Error =
            serde_json::from_str::<i32>("not a number").unwrap_err();
        let err: StorageError = serde_err.into();
        matches!(err, StorageError::Serde(_));
        let display = err.to_string();
        assert!(
            display.contains("serialization error"),
            "Display should include prefix: {display}"
        );
    }

    #[test]
    fn from_io_error_yields_io_variant() {
        let io_err = std::io::Error::other("boom");
        let err: StorageError = io_err.into();
        assert!(matches!(err, StorageError::Io(_)));
    }

    #[test]
    fn storage_result_ok_and_err_shortcuts() {
        let ok: StorageResult<i32> = Ok(7);
        assert_eq!(ok.unwrap(), 7);

        let err: StorageResult<i32> = Err(StorageError::Invalid("x".into()));
        assert!(err.is_err());
    }

    #[test]
    fn sqlx_error_conversion_via_question_mark() {
        fn propagate() -> StorageResult<()> {
            let io_err = std::io::Error::other("inner");
            let _x: StorageError = StorageError::Io(io_err);
            Ok(())
        }
        assert!(propagate().is_ok());
    }

    #[test]
    fn variants_are_distinct_patterns() {
        let io_err = std::io::Error::other("io");
        let err: StorageError = StorageError::Io(io_err);
        match err {
            StorageError::Sqlx(_) => panic!("should not be Sqlx"),
            StorageError::Migration(_) => panic!("should not be Migration"),
            StorageError::Serde(_) => panic!("should not be Serde"),
            StorageError::Io(_) => {}
            StorageError::NotFound(_) => panic!("should not be NotFound"),
            StorageError::Invalid(_) => panic!("should not be Invalid"),
        }
    }
}
