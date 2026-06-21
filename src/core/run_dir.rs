//! Human-readable run directory naming: `{slug}_{unix_timestamp}`.
//!
//! The run's internal identifier stays a UUID v7 (stored in checkpoint.json
//! and event payloads). The on-disk directory name is derived from the
//! workflow name + timestamp for easy scanning and CLI tab-completion.

use std::path::Path;

/// Convert an arbitrary string into a filesystem-safe slug.
///
/// - lowercased
/// - non-alphanumeric runs collapsed to `-`
/// - trimmed of leading/trailing `-`
/// - capped at 50 chars
pub fn slugify(s: &str) -> String {
    let slug: String = s
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .lines()
        .collect::<Vec<&str>>()
        .join("-");
    let collapsed = collapse_dashes(&slug);
    let trimmed = collapsed.trim_matches('-');
    let capped = if trimmed.len() > 50 {
        let mut end = 50;
        while !trimmed.is_char_boundary(end) {
            end -= 1;
        }
        &trimmed[..end]
    } else {
        trimmed
    };
    if capped.is_empty() {
        "run".to_string()
    } else {
        capped.to_string()
    }
}

fn collapse_dashes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_dash {
                result.push(c);
            }
            prev_dash = true;
        } else {
            result.push(c);
            prev_dash = false;
        }
    }
    result
}

/// Derive a slug from the workflow source.
///
/// - `--workflow path` → filename without extension
/// - `--nl "text"`     → slugified text (first 6 words)
/// - inline script     → `maestro-workflow`
pub fn derive_slug(workflow_path: Option<&Path>, nl: Option<&str>) -> String {
    if let Some(path) = workflow_path {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            return slugify(stem);
        }
    }
    if let Some(text) = nl {
        let words: Vec<&str> = text.split_whitespace().take(6).collect();
        return slugify(&words.join(" "));
    }
    "maestro-workflow".to_string()
}

/// Compose a directory name: `{slug}_{timestamp}`.
pub fn compose(slug: &str, unix_ts: u64) -> String {
    format!("{slug}_{unix_ts}")
}

/// Ensure the directory name doesn't collide with an existing directory.
/// Appends `_2`, `_3`, … as needed.
pub fn ensure_unique(base_dir: &Path, dir_name: &str) -> String {
    if !base_dir.join(dir_name).exists() {
        return dir_name.to_string();
    }
    for n in 2..u64::MAX {
        let candidate = format!("{dir_name}_{n}");
        if !base_dir.join(&candidate).exists() {
            return candidate;
        }
    }
    unreachable!("exhausted suffix space");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("deep research v2!"), "deep-research-v2");
        assert_eq!(slugify("  trim  "), "trim");
    }

    #[test]
    fn slugify_collapse() {
        assert_eq!(slugify("a---b"), "a-b");
        assert_eq!(slugify("a   b"), "a-b");
    }

    #[test]
    fn slugify_empty_fallback() {
        assert_eq!(slugify("!!!"), "run");
        assert_eq!(slugify(""), "run");
    }

    #[test]
    fn slugify_long() {
        let long = "a".repeat(100);
        let result = slugify(&long);
        assert_eq!(result.len(), 50);
    }

    #[test]
    fn derive_from_workflow_path() {
        assert_eq!(
            derive_slug(Some(Path::new("scripts/clean.lua")), None),
            "clean"
        );
        assert_eq!(
            derive_slug(Some(Path::new("examples/deep-research.lua")), None),
            "deep-research"
        );
    }

    #[test]
    fn derive_from_nl() {
        let slug = derive_slug(None, Some("research AI trends in 2025"));
        assert_eq!(slug, "research-ai-trends-in-2025");
    }

    #[test]
    fn derive_fallback() {
        assert_eq!(derive_slug(None, None), "maestro-workflow");
    }

    #[test]
    fn compose_format() {
        assert_eq!(compose("clean", 1781980050), "clean_1781980050");
    }

    #[test]
    fn ensure_unique_no_collision() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            ensure_unique(dir.path(), "clean_123"),
            "clean_123"
        );
    }

    #[test]
    fn ensure_unique_with_collision() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("clean_123")).unwrap();
        assert_eq!(
            ensure_unique(dir.path(), "clean_123"),
            "clean_123_2"
        );
    }
}
