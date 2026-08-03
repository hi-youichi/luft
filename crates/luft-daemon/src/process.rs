//! PID file management for daemon discovery.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct PidFile {
    pub pid: u32,
    pub addr: String,
    pub started_at: String,
    pub version: String,
}

/// Return the canonical PID file path: `$LUFT_HOME/daemon.pid` or `~/.luft/daemon.pid`.
pub fn pid_file_path() -> PathBuf {
    if let Ok(home) = std::env::var("LUFT_HOME") {
        return PathBuf::from(home).join("daemon.pid");
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".luft").join("daemon.pid")
}

/// Write the PID file atomically.
pub fn write(pid: u32, addr: &str) -> Result<()> {
    let path = pid_file_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create dir {:?}", parent))?;
    }
    let content = PidFile {
        pid,
        addr: addr.to_string(),
        started_at: chrono::Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let json = serde_json::to_string_pretty(&content)?;
    let tmp = path.with_extension("pid.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Read and parse the PID file. Returns `Ok(None)` if it doesn't exist.
pub fn read() -> Result<Option<PidFile>> {
    let path = pid_file_path();
    if !path.exists() {
        return Ok(None);
    }
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("read pid file {:?}", path))?;
    let parsed: PidFile = serde_json::from_str(&data)
        .with_context(|| format!("parse pid file {:?}", path))?;
    Ok(Some(parsed))
}

/// Delete the PID file (if present).
pub fn remove() {
    let _ = std::fs::remove_file(pid_file_path());
}

/// Check whether a process with the given PID is alive.
pub fn is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: kill(0) is a standard liveness check
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        unsafe {
            let h: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return false;
            }
            let _ = CloseHandle(h);
            true
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

/// Returns `Ok(addr)` if a live daemon is found, `Ok(None)` otherwise.
pub fn discover() -> Result<Option<String>> {
    match read()? {
        Some(pf) if is_alive(pf.pid) => Ok(Some(pf.addr)),
        Some(_) => {
            // stale PID file
            remove();
            Ok(None)
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn pid_file_missing() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("LUFT_HOME", tmp.path());
        assert!(read().unwrap().is_none());
        assert!(discover().unwrap().is_none());
    }

    #[test]
    fn pid_file_write_read_roundtrip() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("LUFT_HOME", tmp.path());
        write(99999, "127.0.0.1:9999").unwrap();
        let pf = read().unwrap().unwrap();
        assert_eq!(pf.pid, 99999);
        assert_eq!(pf.addr, "127.0.0.1:9999");
    }

    #[test]
    fn pid_file_stale_removed_on_discover() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("LUFT_HOME", tmp.path());
        write(99999, "127.0.0.1:9999").unwrap();
        assert!(pid_file_path().exists());
        let result = discover().unwrap();
        assert!(result.is_none());
        assert!(!pid_file_path().exists());
    }

    #[test]
    fn pid_file_corrupt_json() {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("LUFT_HOME", tmp.path());
        std::fs::write(pid_file_path(), "not json").unwrap();
        assert!(read().is_err());
    }
}
