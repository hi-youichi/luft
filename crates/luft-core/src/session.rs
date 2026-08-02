//! Process-local mapping between Luft session ids and backend protocol ids.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRecord {
    pub backend_id: String,
    pub protocol_session_id: String,
}

static REGISTRY: OnceLock<RwLock<HashMap<String, SessionRecord>>> = OnceLock::new();

fn registry() -> &'static RwLock<HashMap<String, SessionRecord>> {
    REGISTRY.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Create a new opaque Luft id for a backend protocol session.
pub fn register_session(backend_id: &str, protocol_session_id: &str) -> String {
    let session_id = format!("luft-{}", Uuid::now_v7());
    registry().write().expect("session registry poisoned").insert(
        session_id.clone(),
        SessionRecord {
            backend_id: backend_id.to_string(),
            protocol_session_id: protocol_session_id.to_string(),
        },
    );
    session_id
}

/// Restore a checkpointed opaque id into the current process.
pub fn restore_session(
    session_id: &str,
    backend_id: &str,
    protocol_session_id: &str,
) {
    registry().write().expect("session registry poisoned").insert(
        session_id.to_string(),
        SessionRecord {
            backend_id: backend_id.to_string(),
            protocol_session_id: protocol_session_id.to_string(),
        },
    );
}

/// Resolve an opaque id for a specific backend.
pub fn resolve_session(session_id: &str, backend_id: &str) -> Option<SessionRecord> {
    registry()
        .read()
        .expect("session registry poisoned")
        .get(session_id)
        .filter(|record| record.backend_id == backend_id)
        .cloned()
}

/// Remove a session after workflow termination.
pub fn remove_session(session_id: &str) {
    registry()
        .write()
        .expect("session registry poisoned")
        .remove(session_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_resolve_opaque_session() {
        let id = register_session("acp", "protocol-1");
        assert!(id.starts_with("luft-"));
        assert_eq!(
            resolve_session(&id, "acp").unwrap().protocol_session_id,
            "protocol-1"
        );
        assert!(resolve_session(&id, "other").is_none());
        remove_session(&id);
        assert!(resolve_session(&id, "acp").is_none());
    }
}
