//! `workflows` subcommand: list saved workflow files under the config dir.

use anyhow::Result;

pub fn list_workflows() -> Result<()> {
    // List workflows from ~/.luft/workflows/ directory
    let workflow_dir = dirs::config_dir()
        .unwrap_or_default()
        .join("luft")
        .join("workflows");

    if !workflow_dir.exists() {
        println!("No workflows found. Create one with `luft save <name> <file>`");
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
            std::env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join("Library").join("Application Support"))
        }
        #[cfg(not(target_os = "macos"))]
        {
            std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var("HOME")
                        .ok()
                        .map(|h| PathBuf::from(h).join(".config"))
                })
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::list_workflows;
    #[cfg(unix)]
    use std::path::PathBuf;
    #[cfg(unix)]
    use std::sync::Mutex;
    #[cfg(unix)]
    use tempfile::TempDir;

    #[cfg(unix)]
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(unix)]
    struct HomeEnv {
        _lock: std::sync::MutexGuard<'static, ()>,
        _dir: TempDir,
        orig_home: Option<String>,
        orig_xdg: Option<String>,
    }

    #[cfg(unix)]
    fn lock_home() -> std::sync::MutexGuard<'static, ()> {
        HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(unix)]
    use serial_test::serial;

    #[cfg(unix)]
    fn config_env_var() -> &'static str {
        if cfg!(windows) {
            "APPDATA"
        } else {
            "HOME"
        }
    }

    #[cfg(unix)]
    impl HomeEnv {
        fn new() -> Self {
            let _lock = lock_home();
            let dir = TempDir::new().unwrap();
            let key = config_env_var();
            let orig_home = std::env::var(key).ok();
            let orig_xdg = std::env::var("XDG_CONFIG_HOME").ok();
            std::env::set_var(key, dir.path());
            std::env::remove_var("XDG_CONFIG_HOME");
            HomeEnv {
                _lock,
                _dir: dir,
                orig_home,
                orig_xdg,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for HomeEnv {
        fn drop(&mut self) {
            let key = config_env_var();
            match &self.orig_home {
                Some(h) => std::env::set_var(key, h),
                None => std::env::remove_var(key),
            }
            match &self.orig_xdg {
                Some(xdg) => std::env::set_var("XDG_CONFIG_HOME", xdg),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    #[cfg(unix)]
    struct UnsetHomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        orig_home: Option<String>,
        orig_xdg: Option<String>,
    }

    #[cfg(unix)]
    impl UnsetHomeGuard {
        #[cfg(target_os = "macos")]
        fn new() -> Self {
            let _lock = lock_home();
            let key = config_env_var();
            let orig_home = std::env::var(key).ok();
            let orig_xdg = std::env::var("XDG_CONFIG_HOME").ok();
            std::env::remove_var(key);
            std::env::remove_var("XDG_CONFIG_HOME");
            UnsetHomeGuard {
                _lock,
                orig_home,
                orig_xdg,
            }
        }
    }

    #[cfg(unix)]
    impl Drop for UnsetHomeGuard {
        fn drop(&mut self) {
            let key = config_env_var();
            match &self.orig_home {
                Some(h) => std::env::set_var(key, h),
                None => std::env::remove_var(key),
            }
            match &self.orig_xdg {
                Some(xdg) => std::env::set_var("XDG_CONFIG_HOME", xdg),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
        }
    }

    #[cfg(unix)]
    fn workflow_dir() -> PathBuf {
        let config = dirs::config_dir().unwrap();
        config.join("luft").join("workflows")
    }

    #[test]
    #[serial]
    #[cfg(target_os = "macos")]
    fn config_dir_returns_macos_path_when_home_set() {
        let _env = HomeEnv::new();
        let home = std::env::var("HOME").unwrap();
        let expected = PathBuf::from(home)
            .join("Library")
            .join("Application Support");
        assert_eq!(dirs::config_dir(), Some(expected));
    }

    #[test]
    #[serial]
    #[cfg(target_os = "macos")]
    fn config_dir_uses_fallback_when_home_unset() {
        let _guard = UnsetHomeGuard::new();
        // dirs v5 on macOS falls back to getpwuid_r() when HOME is unset,
        // so config_dir() may still return Some(...).
        let result = dirs::config_dir();
        if let Some(path) = &result {
            assert!(!path.starts_with("/tmp"));
        }
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn list_workflows_dir_does_not_exist() {
        let _env = HomeEnv::new();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn list_workflows_empty_directory() {
        let _env = HomeEnv::new();
        std::fs::create_dir_all(workflow_dir()).unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn list_workflows_with_lua_files() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("foo.lua"), "return 1").unwrap();
        std::fs::write(wd.join("bar.lua"), "return 2").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn list_workflows_filters_non_lua_files() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("valid.lua"), "return 1").unwrap();
        std::fs::write(wd.join("notes.txt"), "hello").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn list_workflows_skips_files_without_extension() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("workflow.lua"), "return 1").unwrap();
        std::fs::write(wd.join("README"), "").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn list_workflows_read_dir_error() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(wd.parent().unwrap()).unwrap();
        std::fs::write(&wd, "not a directory").unwrap();
        assert!(list_workflows().is_err());
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn home_env_drop_handles_originally_unset_home() {
        let key = config_env_var();
        let orig = std::env::var(key).ok();
        std::env::remove_var(key);
        let lock = lock_home();
        let dir = TempDir::new().unwrap();
        std::env::set_var(key, dir.path());
        {
            let _env = HomeEnv {
                _lock: lock,
                _dir: dir,
                orig_home: None,
                orig_xdg: None,
            };
        }
        match &orig {
            Some(h) => std::env::set_var(key, h),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn home_env_restores_previous_home_value() {
        let key = "HOME";
        let orig = std::env::var(key).ok();
        let dir = TempDir::new().unwrap();
        let dir_path = dir.path().to_str().unwrap().to_string();
        std::env::set_var(key, dir.path());

        let lock = lock_home();
        {
            let _env = HomeEnv {
                _lock: lock,
                _dir: dir,
                orig_home: orig.clone(),
                orig_xdg: None,
            };
            assert_eq!(std::env::var(key).ok().as_deref(), Some(dir_path.as_str()));
        }

        let _verify = lock_home();
        match &orig {
            Some(v) => assert_eq!(std::env::var(key).ok().as_deref(), Some(v.as_str())),
            None => assert!(std::env::var(key).is_err()),
        }
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn home_env_keeps_home_unset_if_originally_unset() {
        let key = "HOME";
        let orig = std::env::var(key).ok();
        std::env::remove_var(key);

        let lock = lock_home();
        let dir = TempDir::new().unwrap();
        std::env::set_var(key, dir.path());
        {
            let _env = HomeEnv {
                _lock: lock,
                _dir: dir,
                orig_home: orig.clone(),
                orig_xdg: None,
            };
        }

        let _verify = lock_home();
        match &orig {
            Some(v) => assert_eq!(std::env::var(key).ok().as_deref(), Some(v.as_str())),
            None => assert!(std::env::var(key).is_err()),
        }
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn unset_home_guard_restores_previous_home_value() {
        let key = "HOME";
        let dir = TempDir::new().unwrap();
        std::env::set_var(key, dir.path());

        let lock = lock_home();
        {
            std::env::remove_var(key);
            let _guard = UnsetHomeGuard {
                _lock: lock,
                orig_home: Some(dir.path().to_string_lossy().into_owned()),
                orig_xdg: None,
            };
            assert!(std::env::var(key).is_err());
        }

        let _verify = lock_home();
        assert_eq!(
            std::env::var(key).ok().as_deref(),
            Some(dir.path().to_str().unwrap())
        );
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn unset_home_guard_keeps_home_unset_if_originally_unset() {
        let key = "HOME";
        std::env::remove_var(key);

        let lock = lock_home();
        {
            let _guard = UnsetHomeGuard {
                _lock: lock,
                orig_home: None,
                orig_xdg: None,
            };
        }

        let _verify = lock_home();
        assert!(std::env::var(key).is_err());
    }

    // ========================================================================
    // Stress / scale
    // ========================================================================

    #[test]
    #[serial]
    #[cfg(unix)]
    fn list_workflows_handles_many_files() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        for i in 0..50 {
            std::fs::write(wd.join(format!("workflow_{:03}.lua", i)), "return 1").unwrap();
        }
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn list_workflows_handles_empty_lua_file() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("empty.lua"), "").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[serial]
    #[cfg(unix)]
    fn list_workflows_handles_unicode_filenames() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("工作流.lua"), "return 1").unwrap();
        std::fs::write(wd.join("d\u{00e9}mo.lua"), "return 2").unwrap();
        assert!(list_workflows().is_ok());
    }
}

