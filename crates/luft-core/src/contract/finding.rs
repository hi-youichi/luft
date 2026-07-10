//! Structured finding schema (§1.3) — the data-plane output contract.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A structured finding reported by an agent via MCP `report_finding`.
/// The schema *is* the contract — agents emit these instead of free text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Category, e.g. "missing_auth" / "source".
    pub kind: String,
    pub severity: Severity,
    pub title: String,
    pub detail: String,
    /// Optional file:line locator.
    pub location: Option<Location>,
    /// Supporting evidence / citations.
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Free-form structured extension.
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub file: PathBuf,
    pub line: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_finding() -> Finding {
        Finding {
            kind: "missing_auth".into(),
            severity: Severity::High,
            title: "Endpoint lacks auth".into(),
            detail: "POST /admin has no authentication middleware".into(),
            location: Some(Location {
                file: PathBuf::from("src/api.rs"),
                line: Some(42),
            }),
            evidence: vec!["grep -n auth src/api.rs".into()],
            data: json!({"cwe": "CWE-306"}),
        }
    }

    #[test]
    fn finding_serialize_roundtrip() {
        let f = sample_finding();
        let json = serde_json::to_string(&f).unwrap();
        let back: Finding = serde_json::from_str(&json).unwrap();
        assert_eq!(back.kind, f.kind);
        assert_eq!(back.severity, f.severity);
        assert_eq!(back.title, f.title);
        assert_eq!(back.detail, f.detail);
        assert_eq!(
            serde_json::to_value(back.location.clone()).unwrap(),
            serde_json::to_value(f.location.clone()).unwrap()
        );
        assert_eq!(back.evidence, f.evidence);
        assert_eq!(back.data, f.data);
    }

    #[test]
    fn finding_minimal_optional_fields_default() {
        let json = json!({
            "kind": "source",
            "severity": "info",
            "title": "note",
            "detail": "free text"
        });
        let f: Finding = serde_json::from_value(json).unwrap();
        assert_eq!(f.kind, "source");
        assert_eq!(f.severity, Severity::Info);
        assert!(f.location.is_none());
        assert!(f.evidence.is_empty());
        assert_eq!(f.data, serde_json::Value::Null);
    }

    #[test]
    fn severity_serializes_as_lowercase() {
        for (s, expected) in [
            (Severity::Info, "\"info\""),
            (Severity::Low, "\"low\""),
            (Severity::Medium, "\"medium\""),
            (Severity::High, "\"high\""),
            (Severity::Critical, "\"critical\""),
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), expected);
        }
    }

    #[test]
    fn severity_deserializes_from_lowercase() {
        for (raw, expected) in [
            ("\"info\"", Severity::Info),
            ("\"low\"", Severity::Low),
            ("\"medium\"", Severity::Medium),
            ("\"high\"", Severity::High),
            ("\"critical\"", Severity::Critical),
        ] {
            let s: Severity = serde_json::from_str(raw).unwrap();
            assert_eq!(s, expected);
        }
    }

    #[test]
    fn severity_ordering_is_strict() {
        assert!(Severity::Info < Severity::Low);
        assert!(Severity::Low < Severity::Medium);
        assert!(Severity::Medium < Severity::High);
        assert!(Severity::High < Severity::Critical);
        // Ord consistency: sorted vec round-trips.
        let mut v = vec![
            Severity::Critical,
            Severity::Info,
            Severity::High,
            Severity::Low,
            Severity::Medium,
        ];
        v.sort();
        assert_eq!(
            v,
            vec![
                Severity::Info,
                Severity::Low,
                Severity::Medium,
                Severity::High,
                Severity::Critical
            ]
        );
    }

    #[test]
    fn location_with_line_roundtrip() {
        let loc = Location {
            file: PathBuf::from("/tmp/foo/bar.rs"),
            line: Some(128),
        };
        let json = serde_json::to_string(&loc).unwrap();
        let back: Location = serde_json::from_str(&json).unwrap();
        assert_eq!(
            serde_json::to_value(&back).unwrap(),
            serde_json::to_value(&loc).unwrap()
        );
    }

    #[test]
    fn location_without_line_roundtrip() {
        let loc = Location {
            file: PathBuf::from("README.md"),
            line: None,
        };
        let json = serde_json::to_string(&loc).unwrap();
        let back: Location = serde_json::from_str(&json).unwrap();
        assert_eq!(
            serde_json::to_value(&back).unwrap(),
            serde_json::to_value(&loc).unwrap()
        );
        assert!(back.line.is_none());
    }

    #[test]
    fn finding_clone_preserves_all_fields() {
        let f = sample_finding();
        let cloned = f.clone();
        assert_eq!(cloned.kind, f.kind);
        assert_eq!(cloned.severity, f.severity);
        assert_eq!(cloned.title, f.title);
        assert_eq!(cloned.detail, f.detail);
        assert_eq!(
            serde_json::to_value(cloned.location.clone()).unwrap(),
            serde_json::to_value(f.location.clone()).unwrap()
        );
        assert_eq!(cloned.evidence, f.evidence);
        assert_eq!(cloned.data, f.data);
    }

    #[test]
    fn finding_debug_includes_kind_and_severity() {
        let f = sample_finding();
        let dbg = format!("{:?}", f);
        assert!(dbg.contains("missing_auth"));
        assert!(dbg.contains("High"));
    }

    #[test]
    fn finding_with_empty_evidence_and_null_data_roundtrip() {
        let f = Finding {
            kind: "x".into(),
            severity: Severity::Low,
            title: "t".into(),
            detail: "d".into(),
            location: None,
            evidence: vec![],
            data: serde_json::Value::Null,
        };
        let json = serde_json::to_string(&f).unwrap();
        // Optional fields with serde(default) emit `"evidence":[]` and `"data":null`
        assert!(json.contains("\"evidence\":[]"));
        let back: Finding = serde_json::from_str(&json).unwrap();
        assert_eq!(
            serde_json::to_value(&back).unwrap(),
            serde_json::to_value(&f).unwrap()
        );
    }
}
