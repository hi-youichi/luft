//! Database connection pool + schema migration.

use crate::storage::error::StorageResult;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Default DB path relative to `.maestro/` directory.
pub const DEFAULT_DB_PATH: &str = "maestro.db";

pub type DbPool = SqlitePool;

/// Open (or create) the SQLite database at the given path.
///
/// Configures WAL mode + foreign keys + busy timeout. Runs migrations
/// from the `migrations/` directory at compile time via `sqlx::migrate!`.
pub async fn open_db(path: &Path) -> StorageResult<DbPool> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    Ok(pool)
}

/// Default DB path under a given `.maestro` root.
pub fn default_db_path(maestro_dir: &Path) -> PathBuf {
    maestro_dir.join(DEFAULT_DB_PATH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn open_db_creates_tables() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let pool = open_db(&db_path).await.unwrap();

        // Verify all 7 tables exist.
        let rows = sqlx::query_as::<_, (String,)>(
            "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();

        let names: Vec<String> = rows.into_iter().map(|r| r.0).collect();
        assert!(names.contains(&"runs".to_string()));
        assert!(names.contains(&"phases".to_string()));
        assert!(names.contains(&"agents".to_string()));
        assert!(names.contains(&"turns".to_string()));
        assert!(names.contains(&"spans".to_string()));
        assert!(names.contains(&"findings".to_string()));
        assert!(names.contains(&"events".to_string()));
    }

    #[tokio::test]
    async fn open_db_idempotent_migration() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let _pool1 = open_db(&db_path).await.unwrap();
        // Second open must not re-run migrations or fail.
        let _pool2 = open_db(&db_path).await.unwrap();
    }

    #[tokio::test]
    async fn foreign_keys_enabled() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = open_db(&db_path).await.unwrap();

        let enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(enabled, 1);
    }

    #[tokio::test]
    async fn wal_mode_enabled() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = open_db(&db_path).await.unwrap();

        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[tokio::test]
    async fn missing_parent_dir_is_created() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("a/b/c/test.db");

        // Should not error.
        let _pool = open_db(&nested).await.unwrap();
        assert!(nested.exists());
    }

    #[allow(dead_code)]
    fn _assert_send_sync() {
        fn assert<T: Send + Sync>() {}
        assert::<DbPool>();
    }
}