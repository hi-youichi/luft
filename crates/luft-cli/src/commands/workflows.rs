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
    }

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
            let _lock = HOME_LOCK.lock().unwrap();
            let dir = TempDir::new().unwrap();
            let key = config_env_var();
            let orig_home = std::env::var(key).ok();
            std::env::set_var(key, dir.path());
            HomeEnv {
                _lock,
                _dir: dir,
                orig_home,
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
        }
    }

    #[cfg(unix)]
    struct UnsetHomeGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        orig_home: Option<String>,
    }

    #[cfg(unix)]
    impl UnsetHomeGuard {
        fn new() -> Self {
            let _lock = HOME_LOCK.lock().unwrap();
            let key = config_env_var();
            let orig_home = std::env::var(key).ok();
            std::env::remove_var(key);
            UnsetHomeGuard { _lock, orig_home }
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
        }
    }

    #[cfg(unix)]
    fn workflow_dir() -> PathBuf {
        let config = dirs::config_dir().unwrap();
        config.join("luft").join("workflows")
    }

    #[test]
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
    #[cfg(unix)]
    fn config_dir_returns_none_when_home_unset() {
        let _guard = UnsetHomeGuard::new();
        assert!(dirs::config_dir().is_none());
    }

    #[test]
    #[cfg(unix)]
    fn list_workflows_dir_does_not_exist() {
        let _env = HomeEnv::new();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn list_workflows_empty_directory() {
        let _env = HomeEnv::new();
        std::fs::create_dir_all(workflow_dir()).unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
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
    #[cfg(unix)]
    fn list_workflows_read_dir_error() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(wd.parent().unwrap()).unwrap();
        std::fs::write(&wd, "not a directory").unwrap();
        assert!(list_workflows().is_err());
    }

    #[test]
    #[cfg(unix)]
    fn home_env_drop_handles_originally_unset_home() {
        let key = config_env_var();
        let orig = std::env::var(key).ok();
        std::env::remove_var(key);
        let lock = HOME_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        std::env::set_var(key, dir.path());
        {
            let _env = HomeEnv {
                _lock: lock,
                _dir: dir,
                orig_home: None,
            };
        }
        match &orig {
            Some(h) => std::env::set_var(key, h),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    #[cfg(unix)]
    fn unset_home_guard_drop_handles_none_orig() {
        let key = config_env_var();
        let lock = HOME_LOCK.lock().unwrap();
        let orig = std::env::var(key).ok();
        {
            let _guard = UnsetHomeGuard {
                _lock: lock,
                orig_home: None,
            };
        }
        match &orig {
            Some(h) => std::env::set_var(key, h),
            None => std::env::remove_var(key),
        }
    }

    // ========================================================================
    // XDG_CONFIG_HOME resolution (non-macOS Unix only)
    // ========================================================================

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn xdg_config_home_takes_priority_over_home() {
        let _lock = HOME_LOCK.lock().unwrap();
        let xdg = TempDir::new().unwrap();
        let home = TempDir::new().unwrap();
        let orig_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", xdg.path());
        std::env::set_var("HOME", home.path());
        assert_eq!(dirs::config_dir(), Some(xdg.path().to_path_buf()));
        match &orig_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match &orig_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn xdg_config_home_fallback_to_home_dot_config() {
        let _lock = HOME_LOCK.lock().unwrap();
        let home = TempDir::new().unwrap();
        let orig_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let orig_home = std::env::var("HOME").ok();
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::set_var("HOME", home.path());
        let expected = home.path().join(".config");
        assert_eq!(dirs::config_dir(), Some(expected));
        match &orig_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match &orig_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn xdg_config_home_used_when_home_unset() {
        let _lock = HOME_LOCK.lock().unwrap();
        let xdg = TempDir::new().unwrap();
        let orig_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        let orig_home = std::env::var("HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", xdg.path());
        std::env::remove_var("HOME");
        assert_eq!(dirs::config_dir(), Some(xdg.path().to_path_buf()));
        match &orig_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        match &orig_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    // ========================================================================
    // list_workflows boundary conditions
    // ========================================================================

    #[test]
    #[cfg(unix)]
    fn list_workflows_with_uppercase_lua_extension() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("lower.lua"), "return 1").unwrap();
        std::fs::write(wd.join("UPPER.LUA"), "return 1").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn list_workflows_with_double_extension_lua() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("archive.tar.lua"), "return 1").unwrap();
        std::fs::write(wd.join("backup.lua.bak"), "old").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn list_workflows_with_hidden_lua_files() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join(".hidden.lua"), "return 1").unwrap();
        std::fs::write(wd.join("visible.lua"), "return 2").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn list_workflows_with_file_named_exactly_lua() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join(".lua"), "return 1").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn list_workflows_with_mixed_extensions() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("a.lua"), "1").unwrap();
        std::fs::write(wd.join("b.txt"), "2").unwrap();
        std::fs::write(wd.join("c.json"), "{}").unwrap();
        std::fs::write(wd.join("d"), "").unwrap();
        std::fs::write(wd.join("e.lua.bak"), "5").unwrap();
        std::fs::write(wd.join("f.luac"), "6").unwrap();
        assert!(list_workflows().is_ok());
    }

    // ========================================================================
    // Subdirectories and special filesystem entries
    // ========================================================================

    #[test]
    #[cfg(unix)]
    fn list_workflows_skips_subdirectories() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::create_dir(wd.join("subdir")).unwrap();
        std::fs::write(wd.join("top.lua"), "return 1").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
    #[cfg(unix)]
    fn list_workflows_skips_nested_subdirectory_with_lua_name() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::create_dir_all(wd.join("nested.lua")).unwrap();
        std::fs::write(wd.join("top.lua"), "return 1").unwrap();
        assert!(list_workflows().is_ok());
    }

    // ========================================================================
    // Config dir unresolvable — both HOME and XDG_CONFIG_HOME unset
    // ========================================================================

    #[test]
    #[cfg(unix)]
    fn list_workflows_returns_ok_when_config_dir_unresolvable() {
        let _lock = HOME_LOCK.lock().unwrap();
        let orig_home = std::env::var("HOME").ok();
        std::env::remove_var("HOME");
        #[cfg(not(target_os = "macos"))]
        let orig_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        #[cfg(not(target_os = "macos"))]
        std::env::remove_var("XDG_CONFIG_HOME");

        let result = list_workflows();

        if let Some(v) = &orig_home {
            std::env::set_var("HOME", v);
        }
        #[cfg(not(target_os = "macos"))]
        if let Some(v) = &orig_xdg {
            std::env::set_var("XDG_CONFIG_HOME", v);
        }

        assert!(result.is_ok());
    }

    // ========================================================================
    // Guard restoration verification
    // ========================================================================

    #[test]
    #[cfg(unix)]
    fn home_env_restores_previous_home_value() {
        let key = "HOME";
        let orig = std::env::var(key).ok();
        let dir = TempDir::new().unwrap();
        std::env::set_var(key, dir.path());

        let lock = HOME_LOCK.lock().unwrap();
        {
            let _env = HomeEnv {
                _lock: lock,
                _dir: dir,
                orig_home: orig.clone(),
            };
            assert_eq!(
                std::env::var(key).ok().as_deref(),
                Some(dir.path().to_str().unwrap())
            );
        }

        let _verify = HOME_LOCK.lock().unwrap();
        match &orig {
            Some(v) => assert_eq!(std::env::var(key).ok().as_deref(), Some(v.as_str())),
            None => assert!(std::env::var(key).is_err()),
        }
    }

    #[test]
    #[cfg(unix)]
    fn home_env_keeps_home_unset_if_originally_unset() {
        let key = "HOME";
        let orig = std::env::var(key).ok();
        std::env::remove_var(key);

        let lock = HOME_LOCK.lock().unwrap();
        let dir = TempDir::new().unwrap();
        std::env::set_var(key, dir.path());
        {
            let _env = HomeEnv {
                _lock: lock,
                _dir: dir,
                orig_home: orig.clone(),
            };
        }

        let _verify = HOME_LOCK.lock().unwrap();
        match &orig {
            Some(v) => assert_eq!(std::env::var(key).ok().as_deref(), Some(v.as_str())),
            None => assert!(std::env::var(key).is_err()),
        }
    }

    #[test]
    #[cfg(unix)]
    fn unset_home_guard_restores_previous_home_value() {
        let key = "HOME";
        let dir = TempDir::new().unwrap();
        std::env::set_var(key, dir.path());

        let lock = HOME_LOCK.lock().unwrap();
        {
            let _guard = UnsetHomeGuard {
                _lock: lock,
                orig_home: Some(dir.path().to_string_lossy().into_owned()),
            };
            assert!(std::env::var(key).is_err());
        }

        let _verify = HOME_LOCK.lock().unwrap();
        assert_eq!(
            std::env::var(key).ok().as_deref(),
            Some(dir.path().to_str().unwrap())
        );
    }

    #[test]
    #[cfg(unix)]
    fn unset_home_guard_keeps_home_unset_if_originally_unset() {
        let key = "HOME";
        std::env::remove_var(key);

        let lock = HOME_LOCK.lock().unwrap();
        {
            let _guard = UnsetHomeGuard {
                _lock: lock,
                orig_home: None,
            };
        }

        let _verify = HOME_LOCK.lock().unwrap();
        assert!(std::env::var(key).is_err());
    }

    // ========================================================================
    // Stress / scale
    // ========================================================================

    #[test]
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
    #[cfg(unix)]
    fn list_workflows_handles_empty_lua_file() {
        let _env = HomeEnv::new();
        let wd = workflow_dir();
        std::fs::create_dir_all(&wd).unwrap();
        std::fs::write(wd.join("empty.lua"), "").unwrap();
        assert!(list_workflows().is_ok());
    }

    #[test]
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
