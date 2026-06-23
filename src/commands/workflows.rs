//! `workflows` subcommand: list saved workflow files under the config dir.

use anyhow::Result;

pub fn list_workflows() -> Result<()> {
    // List workflows from ~/.maestro/workflows/ directory
    let workflow_dir = dirs::config_dir()
        .unwrap_or_default()
        .join("maestro")
        .join("workflows");

    if !workflow_dir.exists() {
        println!("No workflows found. Create one with `maestro save <name> <file>`");
        return Ok(());
    }

    println!("Available workflows:");
    for entry in std::fs::read_dir(workflow_dir)? {
        let entry = entry?;
        if let Some(ext) = entry.path().extension() {
            if ext == "lua" {
                println!("  - {}", entry.file_name().to_string_lossy());
            }
        }
    }

    Ok(())
}

// Minimal stand-in for the `dirs` crate's `config_dir`, kept inline to avoid
// pulling in the dependency for a single lookup.
mod dirs {
    use std::path::PathBuf;

    /// macOS: ~/Library/Application Support
    /// Linux: ~/.config or $XDG_CONFIG_HOME
    pub fn config_dir() -> Option<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Library").join("Application Support"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use tempfile::TempDir;

    static HOME_LOCK: Mutex<()> = Mutex::new(());

    struct HomeEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        _dir: TempDir,
        orig_home: Option<String>,
    }

    impl HomeEnv {
        fn new() -> Self {
            let _lock = HOME_LOCK.lock().unwrap();
            let dir = TempDir::new().unwrap();
            let orig_home = std::env::var("HOME").ok();
            std::env::set_var("HOME", dir.path());
            HomeEnv { _lock, _dir: dir, orig_home }
        }
    }

    impl Drop for HomeEnv {
        fn drop(&mut self) {
            match &self.orig_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    struct UnsetHomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        orig_home: Option<String>,
    }

    impl UnsetHomeGuard {
        fn new() -> Self {
            let _lock = HOME_LOCK.lock().unwrap();
            let orig_home = std::env::var("HOME").ok();
            std::env::remove_var("HOME");
            UnsetHomeGuard { _lock, orig_home }
        }
    }

    impl Drop for UnsetHomeGuard {
        fn drop(&mut self) {
            match &self.orig_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
    }

    fn workflow_dir() -> PathBuf {
        let config = dirs::config_dir().unwrap();
        config.join("maestro").join("workflows")
    }

    #[test]
    fn config_dir_returns_macos_path_when_home_set() {
        let _env = HomeEnv::new();
        let home = std::env::var("HOME").unwrap();
        let expected = PathBuf::from(home)
            .join("Library")
            .join("Application Support");
        assert_eq!(dirs::config_dir(), Some(expected));
    }

    #[test]
    fn config_dir_returns_none_when_home_unset() {
        let _guard = UnsetHomeGuard::new();
        assert!(dirs::config_dir().is_none());
    }

    #[test]
    fn list_workflows_dir_does_not_exist() {
        let _env = HomeEnv::new();
        assert!(list_workflows().is_ok());
    }

    #[test]
    fn list_workflows_empty_directory() {
        let _env = HomeEnv::new();
        std::fs::create_dir_all(workflow_dir()).unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    fn list_workflows_with_lua_files() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("foo.lua"), "return 1").unwrap();
        std::fs::write(wd.join("bar.lua"), "return 2").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    fn list_workflows_filters_non_lua_files() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("valid.lua"), "return 1").unwrap();
        std::fs::write(wd.join("notes.txt"), "hello").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    fn list_workflows_skips_files_without_extension() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("workflow.lua"), "return 1").unwrap();
        std::fs::write(wd.join("README"), "").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    fn list_workflows_read_dir_error() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(wd.parent().unwrap()).unwrap();
        std::fs::write(&wd, "not a directory").unwrap();
        assert!(list_workflows().is_err());
    }

    #[test]
    fn home_env_drop_handles_originally_unset_home() {
        let orig = std::env::var("HOME").ok();
        std::env::remove_var("HOME");
        let lock = HOME_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        std::env::set_var("HOME", dir.path());
        {
            let _env = HomeEnv { _lock: lock, _dir: dir, orig_home: None };
        }
        match &orig {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn unset_home_guard_drop_handles_none_orig() {
        let lock = HOME_LOCK.lock().unwrap();
        let orig = std::env::var("HOME").ok();
        {
            let _guard = UnsetHomeGuard { _lock: lock, orig_home: None };
        }
        match &orig {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}
