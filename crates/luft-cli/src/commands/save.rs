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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_workflow_returns_err() {
        let result = save_workflow("my-workflow", Path::new("/tmp/out.lua"));
        assert!(result.is_err(), "save_workflow must return an error");
    }

    #[test]
    fn save_workflow_error_mentions_name() {
        let err = save_workflow("alpha", Path::new("/tmp/out.lua")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("alpha"),
            "error message should mention the workflow name, got: {msg}"
        );
    }

    #[test]
    fn save_workflow_error_mentions_output_path() {
        let err = save_workflow("wf", Path::new("/var/tmp/output.lua")).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("output.lua") || msg.contains("/var/tmp"),
            "error message should mention the output path, got: {msg}"
        );
    }

    #[test]
    fn save_workflow_error_says_not_implemented() {
        let err = save_workflow("n", Path::new("o")).unwrap_err();
        assert!(
            err.to_string().contains("not implemented"),
            "error message should explicitly say 'not implemented', got: {err}"
        );
    }

    #[test]
    fn save_workflow_with_empty_name_still_fails() {
        let result = save_workflow("", Path::new("/tmp/out.lua"));
        assert!(result.is_err());
    }

    #[test]
    fn save_workflow_with_relative_path_still_fails() {
        let result = save_workflow("n", Path::new("relative/path.lua"));
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("relative/path.lua"));
    }

    #[test]
    fn save_workflow_error_is_chain() {
        // anyhow::Error::to_string() is the top-level message
        let err = save_workflow("foo", Path::new("bar")).unwrap_err();
        let s = format!("{err}");
        assert!(!s.is_empty(), "error message must not be empty");
    }

    #[test]
    fn save_workflow_downcast_ref_to_dyn_error() {
        // Ensure the returned anyhow::Error behaves like a standard Error trait object.
        let err = save_workflow("wf", Path::new("/tmp/out")).unwrap_err();
        let dyn_err: &dyn std::error::Error = err.as_ref();
        assert!(!dyn_err.to_string().is_empty());
    }
}
