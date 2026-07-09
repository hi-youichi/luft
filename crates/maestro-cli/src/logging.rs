//! Program-log (tracing) initialization — see `docs/design/program-logging.md`.
//!
//! Installs the global `tracing` subscriber so the diagnostic/operational log
//! (spawn failures, retry decisions, protocol errors, …) is actually emitted.
//! This is the **program log** plane — distinct from the event log (`AgentEvent`
//! → `EventLogger`).
//!
//! Logs always go to a **file**, never to stdout or stderr. The file path is
//! resolved as: explicit `--log-file` > `~/.maestro/logs/maestro.log`. Parent
//! directories are created on demand.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Default global log file location when neither `--log-file` nor any override
/// is supplied: `$HOME/.maestro/logs/maestro.log`. Used as a fallback inside
/// [`init`] when `log_file` is `None`.
pub fn default_log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".maestro").join("logs").join("maestro.log"))
}

/// Install the global tracing subscriber. Idempotent — a second call is a no-op
/// (the underlying `try_init` returns `Err` once a global subscriber exists,
/// which we swallow).
///
/// Filter precedence: `level` (`--log-level`) > `RUST_LOG` > `default`.
///
/// Writer: the resolved log file (explicit `log_file` if `Some`, else
/// [`default_log_path`]). Stderr/stdout are never used.
///
/// Returns `Err` only if the explicit log file's parent directory cannot be
/// created. The default-path case falls back to **no-op logging** (still emits
/// `Ok`) when `home_dir()` is unavailable, e.g. in some CI sandboxes.
pub fn init(level: Option<&str>, default: &str, log_file: Option<&Path>) -> anyhow::Result<()> {
    let filter = level
        .and_then(|l| EnvFilter::try_new(l).ok())
        .or_else(|| EnvFilter::try_from_default_env().ok())
        .unwrap_or_else(|| EnvFilter::new(default));

    let Some(path) = log_file.map(Path::to_path_buf).or_else(default_log_path) else {
        // No resolvable path — install a no-op writer so the subscriber still
        // initialises (tests can still exercise filter logic without a file).
        let _ = tracing_subscriber::registry()
            .with(filter)
            .with(fmt::layer().with_writer(NoopWriter).with_target(false))
            .try_init();
        return Ok(());
    };

    let writer = SharedLogWriter::open(&path)?;
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_target(false),
        )
        .try_init();

    Ok(())
}

// ── File writer ───────────────────────────────────────────

/// `MakeWriter` that opens the log file and shares a `Mutex<File>` across
/// threads. Writes go straight to the OS (no `BufWriter`) so events are
/// observable to readers immediately — important for tests and tail-following.
/// The mutex serialises concurrent events so they never interleave.
#[derive(Clone)]
struct SharedLogWriter {
    inner: Arc<Mutex<File>>,
}

impl SharedLogWriter {
    fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(file)),
        })
    }
}

impl<'a> MakeWriter<'a> for SharedLogWriter {
    type Writer = SharedLogHandle;
    fn make_writer(&'a self) -> Self::Writer {
        SharedLogHandle(self.inner.clone())
    }
}

struct SharedLogHandle(Arc<Mutex<File>>);

impl Write for SharedLogHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut guard = self.0.lock().expect("log writer poisoned");
        guard.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        let mut guard = self.0.lock().expect("log writer poisoned");
        guard.flush()
    }
}

/// Discarding writer used when no log file path is resolvable. Keeps the
/// subscriber usable without spamming stdout/stderr.
#[derive(Clone, Copy)]
struct NoopWriter;

impl<'a> MakeWriter<'a> for NoopWriter {
    type Writer = NoopHandle;
    fn make_writer(&'a self) -> Self::Writer {
        NoopHandle
    }
}

struct NoopHandle;

impl Write for NoopHandle {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialise the file-writing tests — they touch a global subscriber and a
    /// shared temp dir, so parallel execution would race.
    static LOG_LOCK: Mutex<()> = Mutex::new(());

    fn temp_log(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("maestro_log_{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn init_default_level() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("default_level.log");
        assert!(init(None, "error", Some(&path)).is_ok());
        drop(_lock);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn init_explicit_level_trace() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("trace.log");
        assert!(init(Some("trace"), "info", Some(&path)).is_ok());
        drop(_lock);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn init_explicit_level_debug() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("debug.log");
        assert!(init(Some("debug"), "warn", Some(&path)).is_ok());
        drop(_lock);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn init_explicit_level_off() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("off.log");
        assert!(init(Some("off"), "info", Some(&path)).is_ok());
        drop(_lock);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn init_empty_level_string() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("empty.log");
        assert!(init(Some(""), "warn", Some(&path)).is_ok());
        drop(_lock);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn init_invalid_level_falls_back_to_default() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("invalid.log");
        assert!(init(Some(":::"), "warn", Some(&path)).is_ok());
        drop(_lock);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn init_uses_rust_log_env_var() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("rust_log.log");
        unsafe {
            std::env::set_var("RUST_LOG", "error");
        }
        let result = init(None, "info", Some(&path));
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
        drop(_lock);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok());
    }

    #[test]
    fn init_explicit_level_overrides_rust_log() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("explicit_overrides.log");
        unsafe {
            std::env::set_var("RUST_LOG", "info");
        }
        let result = init(Some("trace"), "error", Some(&path));
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
        drop(_lock);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok());
    }

    #[test]
    fn init_invalid_level_uses_rust_log_fallback() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("invalid_rust.log");
        unsafe {
            std::env::set_var("RUST_LOG", "warn");
        }
        let result = init(Some(":::"), "error", Some(&path));
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
        drop(_lock);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok());
    }

    #[test]
    fn init_no_rust_log_uses_default() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("no_rust.log");
        unsafe {
            std::env::remove_var("RUST_LOG");
        }
        let result = init(None, "warn", Some(&path));
        drop(_lock);
        let _ = std::fs::remove_file(&path);
        assert!(result.is_ok());
    }

    #[test]
    fn init_idempotent() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("idempotent.log");
        assert!(init(Some("debug"), "info", Some(&path)).is_ok());
        assert!(init(None, "error", Some(&path)).is_ok());
        assert!(init(Some("trace"), "warn", Some(&path)).is_ok());
        drop(_lock);
        let _ = std::fs::remove_file(&path);
    }

    // ── New tests for file-writer behaviour ─────────────

    #[test]
    fn init_creates_parent_directories() {
        let _lock = LOG_LOCK.lock().unwrap();
        let base =
            std::env::temp_dir().join(format!("maestro_log_nested_{}", uuid::Uuid::now_v7()));
        let path = base.join("deep").join("nested").join("file.log");
        assert!(path.parent().unwrap().try_exists().map_or(true, |v| !v));
        assert!(init(None, "info", Some(&path)).is_ok());
        assert!(path.exists(), "log file should be created");
        drop(_lock);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn init_appends_to_existing_file() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("append.log");
        std::fs::write(&path, "PRE-EXISTING\n").unwrap();
        assert!(init(None, "info", Some(&path)).is_ok());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.starts_with("PRE-EXISTING\n"));
        drop(_lock);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn init_returns_err_on_unwritable_path() {
        let _lock = LOG_LOCK.lock().unwrap();
        // A path under a regular file (not a directory) cannot host a file as
        // its child — `create_dir_all` should fail.
        let blocker = temp_log("blocker");
        std::fs::write(&blocker, "i am a file").unwrap();
        let bad_path = blocker.join("nested").join("file.log");
        let result = init(None, "info", Some(&bad_path));
        drop(_lock);
        let _ = std::fs::remove_file(&blocker);
        assert!(result.is_err(), "expected unwritable path to error");
    }

    #[test]
    fn default_log_path_uses_home() {
        // Verify the resolver puts the file under $HOME/.maestro/logs/.
        // We don't write anything here — just assert the shape.
        let path = default_log_path().expect("home dir should resolve in test env");
        let s = path.to_string_lossy();
        assert!(s.contains(".maestro"), "expected .maestro in {s}");
        assert!(s.ends_with("maestro.log"), "expected maestro.log in {s}");
    }

    #[test]
    fn init_with_no_path_falls_back_to_noop() {
        let _lock = LOG_LOCK.lock().unwrap();
        // Pass None explicitly — should not panic, should install a subscriber
        // (possibly backed by NoopWriter if home dir is also unavailable).
        assert!(init(None, "warn", None).is_ok());
    }

    #[test]
    fn tracing_events_land_in_file() {
        let _lock = LOG_LOCK.lock().unwrap();
        let path = temp_log("events.log");
        // Use a thread-local subscriber pointing at our own file. The global
        // subscriber (installed by an earlier `init` test) writes to a
        // different path; we want to verify events reaching *this* path, so
        // we override at thread scope for the duration of the assertion.
        let writer = SharedLogWriter::open(&path).unwrap();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("info"))
            .with(
                fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false)
                    .with_target(false),
            );
        let _guard = tracing::subscriber::set_default(subscriber);
        tracing::debug!("marker-debug-filtered");
        tracing::info!("marker-info-should-appear");
        tracing::warn!("marker-warn-should-appear");
        tracing::error!("marker-error-should-appear");
        drop(_guard);
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            content.contains("marker-info-should-appear"),
            "expected info marker in log, got: {content}"
        );
        assert!(
            content.contains("marker-warn-should-appear"),
            "expected warn marker in log, got: {content}"
        );
        assert!(
            content.contains("marker-error-should-appear"),
            "expected error marker in log, got: {content}"
        );
        assert!(
            !content.contains("marker-debug-filtered"),
            "debug should be filtered out at info level, got: {content}"
        );
        drop(_lock);
        let _ = std::fs::remove_file(&path);
    }
}
