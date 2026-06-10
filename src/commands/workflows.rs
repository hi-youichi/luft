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
