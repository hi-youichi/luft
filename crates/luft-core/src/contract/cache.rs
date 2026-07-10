//! Stable agent cache key (§1.5) — the basis for `--resume` reuse.

use crate::contract::ids::PhaseId;
use blake3::Hasher;
use unicode_normalization::UnicodeNormalization;

/// Field separator for `agent_cache_key`. A zero byte is safe because BLAKE3
/// hashes raw bytes — it cannot appear in any valid `backend_id`, `model`, or
/// normalized `prompt`, so it cleanly delimits fields and prevents
/// concatenation collisions (e.g. `("ab", "cd")` vs `("a", "bcd")`).
const SEP: u8 = 0;

/// Normalise a prompt for cache keying: NFC, unify line endings, collapse
/// whitespace. Deliberately conservative — only removes formatting noise, not
/// semantic differences (v0.1 trade-off; v0.2 may add similarity matching).
fn normalize_prompt(prompt: &str) -> String {
    prompt
        .nfc()
        .collect::<String>()
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Deterministic cache key: `blake3(backend ++ model ++ normalized_prompt ++ phase)`.
/// `\0` separators prevent field-concatenation collisions.
pub fn agent_cache_key(
    backend_id: &str,
    model: Option<&str>,
    prompt: &str,
    phase: PhaseId,
) -> String {
    let mut h = Hasher::new();
    h.update(backend_id.as_bytes());
    h.update(&[SEP]);
    h.update(model.unwrap_or("").as_bytes());
    h.update(&[SEP]);
    h.update(normalize_prompt(prompt).as_bytes());
    h.update(&[SEP]);
    // Big-endian encodes `phase` identically on every architecture, so cache
    // keys stay stable across platforms for `--resume`. Do NOT switch to
    // `to_ne_bytes()` — keys would silently diverge on big-endian hosts.
    h.update(&phase.to_be_bytes());
    h.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_stable_and_distinct() {
        let a = agent_cache_key("opencode", Some("gpt"), "audit file", 1);
        let b = agent_cache_key("opencode", Some("gpt"), "audit file", 1);
        assert_eq!(a, b, "same inputs must yield same key");

        assert_ne!(a, agent_cache_key("opencode", Some("gpt"), "audit file", 2));
        assert_ne!(a, agent_cache_key("codex", Some("gpt"), "audit file", 1));
        assert_ne!(a, agent_cache_key("opencode", None, "audit file", 1));
    }

    #[test]
    fn whitespace_is_normalized() {
        assert_eq!(
            agent_cache_key("b", None, "  foo\r\nbar  ", 0),
            agent_cache_key("b", None, "foo bar", 0),
        );
    }
}
