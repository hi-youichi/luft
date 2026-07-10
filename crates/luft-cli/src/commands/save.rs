//! `save` subcommand (not yet implemented).

use anyhow::Result;
use std::path::Path;

pub fn save_workflow(name: &str, output: &Path) -> Result<()> {
    // Not implemented yet — fail loudly instead of printing a false success.
    anyhow::bail!(
        "`save` is not implemented yet (would save workflow '{}' to {})",
        name,
        output.display()
    )
}
