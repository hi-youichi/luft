//! # luft-skills
//!
//! Compiled-in workflow authoring skill for Luft agents.
//!
//! This crate owns the skill content (Markdown) and exposes it as:
//! - [`WORKFLOW_SKILL`] - a [`luft_core::Skill`] struct for library consumers
//! - [`LUA_DSL_REFERENCE`] - the full reassembled reference string (main + all
//!   references concatenated), used as the planner system prompt
//! - [`write_to_dir`] - write the skill files to a directory on disk
//!
//! Previously this content lived in `luft-planner`; it was extracted into its
//! own crate so that `luft-cli`'s install command and `luft-adapters`' runtime
//! injection can both depend on it without pulling in the planner's heavier
//! dependencies (mlua, tokio, etc.).

use luft_core::Skill;
use std::path::Path;

const SKILL_MAIN: &str = include_str!("skill/main.md");
const REF_ARCHITECTURE_HEADER: &str = include_str!("skill/references/architecture-header.md");
const REF_PRIMITIVES: &str = include_str!("skill/references/primitives.md");
const REF_AGENT_PROMPTS: &str = include_str!("skill/references/agent-prompts.md");
const REF_TASK_DECOMPOSITION: &str = include_str!("skill/references/task-decomposition.md");
const REF_ADVERSARIAL_VERIFICATION: &str =
    include_str!("skill/references/adversarial-verification.md");
const REF_EXAMPLES: &str = include_str!("skill/references/examples.md");

/// Full reference, reassembled from the split `skill/` files in the same
/// order as the original monolithic `lua_dsl_reference.md`.
pub const LUA_DSL_REFERENCE: &str = const_format::concatcp!(
    SKILL_MAIN,
    "\n",
    REF_ARCHITECTURE_HEADER,
    "\n",
    REF_PRIMITIVES,
    "\n",
    REF_AGENT_PROMPTS,
    "\n",
    REF_TASK_DECOMPOSITION,
    "\n",
    REF_ADVERSARIAL_VERIFICATION,
    "\n",
    REF_EXAMPLES,
);

/// The Lua DSL reference packaged as a [`luft_core::Skill`] - the library-level
/// hand-off point for any crate that embeds `luft` and wants to teach its own
/// agent how to write Luft workflows (e.g. wrapping it into a richer
/// agent-specific skill format with triggers/tool requirements).
pub const WORKFLOW_SKILL: Skill = Skill {
    name: "workflow",
    description: "Lua DSL reference for writing multi-agent Luft workflows",
    content: SKILL_MAIN,
    references: &[
        ("references/architecture-header.md", REF_ARCHITECTURE_HEADER),
        ("references/primitives.md", REF_PRIMITIVES),
        ("references/agent-prompts.md", REF_AGENT_PROMPTS),
        ("references/task-decomposition.md", REF_TASK_DECOMPOSITION),
        (
            "references/adversarial-verification.md",
            REF_ADVERSARIAL_VERIFICATION,
        ),
        ("references/examples.md", REF_EXAMPLES),
    ],
};

/// Write a [`Skill`] to a directory on disk.
///
/// Creates `SKILL.md` from `skill.content`, then writes each reference file
/// under `references/` relative to `dir`. Parent directories are created as
/// needed.
pub fn write_to_dir(dir: &Path, skill: &Skill) -> std::io::Result<usize> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("SKILL.md"), skill.content)?;
    let mut count = 1;
    for (rel_path, content) in skill.references {
        let path = dir.join(rel_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_skill_wraps_the_dsl_reference() {
        assert_eq!(WORKFLOW_SKILL.name, "workflow");
        assert!(!WORKFLOW_SKILL.description.is_empty());
        assert_eq!(WORKFLOW_SKILL.content, SKILL_MAIN);
        assert!(LUA_DSL_REFERENCE.starts_with(SKILL_MAIN));
        assert_eq!(WORKFLOW_SKILL.references.len(), 6);
    }

    #[test]
    fn split_content_reassembles_to_the_full_reference() {
        for marker in [
            "# Output Format",
            "# Execution Model",
            "# Architecture Header",
            "# Meta Table & Entry Point",
            "# Agent Prompt Quality",
            "# Task Decomposition",
            "# Primitives",
            "# Globals",
            "# Error Handling",
            "# Adversarial Verification Pattern",
            "# Rules",
            "# Example: per-module refactoring",
            "# Example: whole-crate refactoring",
            "# Example: adversarial verification",
        ] {
            assert!(
                LUA_DSL_REFERENCE.contains(marker),
                "missing section after split: {marker}"
            );
        }
        assert!(
            !LUA_DSL_REFERENCE.contains("Maestro"),
            "stale project name survived the split"
        );
        assert!(LUA_DSL_REFERENCE.contains("Luft"));
    }

    #[test]
    fn workflow_skill_references_are_reachable_and_nonempty() {
        for (path, content) in WORKFLOW_SKILL.references {
            assert!(!content.is_empty(), "{path} is empty");
            assert!(
                path.starts_with("references/"),
                "{path} should be under references/"
            );
        }
    }

    #[test]
    fn write_to_dir_creates_skill_and_references() {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("workflow");
        let count = write_to_dir(&dir, &WORKFLOW_SKILL).unwrap();
        assert!(dir.join("SKILL.md").exists());
        assert!(dir.join("references/primitives.md").exists());
        assert_eq!(count, 7); // 1 SKILL.md + 6 references
    }
}
