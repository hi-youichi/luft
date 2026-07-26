//! Skill contract (§1.5) — reusable agent-facing guidance, shared at the
//! library level.
//!
//! A [`Skill`] bundles markdown content (and optional reference files) that a
//! *consuming* crate embeds via `include_str!` and hands to whatever agent it
//! drives. `luft-core` only defines the shape; it owns no content itself —
//! see `luft_skills::WORKFLOW_SKILL` for the first instance. Consumers with
//! their own skill format (e.g. a `BuiltinSkill` with triggers and tool
//! requirements) build their richer type from these fields rather than
//! parsing/duplicating the markdown.

/// A named piece of agent-facing guidance, plus any files it references.
///
/// All fields are `&'static str` because every known instance is compiled in
/// via `include_str!` — there is no owned-`String` constructor because a
/// dynamically-built `Skill` has no crate that would outlive the borrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skill {
    /// Short identifier, e.g. `"workflow"`.
    pub name: &'static str,
    /// One-line summary of when this skill applies.
    pub description: &'static str,
    /// The skill body (markdown). No required frontmatter — consumers that
    /// need YAML frontmatter (name/triggers/tags) wrap this content rather
    /// than expecting it embedded here.
    pub content: &'static str,
    /// Bundled reference files as `(relative_path, content)` pairs, e.g.
    /// `("references/examples.md", "...")`. Empty for skills with no
    /// supporting files.
    pub references: &'static [(&'static str, &'static str)],
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: Skill = Skill {
        name: "sample",
        description: "a sample skill",
        content: "# Sample\n\nBody.",
        references: &[("references/extra.md", "extra content")],
    };

    #[test]
    fn fields_are_reachable() {
        assert_eq!(SAMPLE.name, "sample");
        assert_eq!(SAMPLE.description, "a sample skill");
        assert!(SAMPLE.content.contains("Body"));
        assert_eq!(SAMPLE.references.len(), 1);
        assert_eq!(SAMPLE.references[0].0, "references/extra.md");
    }

    #[test]
    fn empty_references_is_valid() {
        const NO_REFS: Skill = Skill {
            name: "bare",
            description: "no references",
            content: "content",
            references: &[],
        };
        assert!(NO_REFS.references.is_empty());
    }

    #[test]
    fn skill_is_copy_and_comparable() {
        let copied = SAMPLE;
        assert_eq!(SAMPLE, copied);
    }
}
