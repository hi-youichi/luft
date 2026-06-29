//! Backend registry (§2.2): backend id → implementation. Registered before a
//! run, read-only during it.

use super::error::SchedulerError;
use crate::core::contract::backend::AgentBackend;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Default)]
pub struct BackendRegistry {
    backends: HashMap<&'static str, Arc<dyn AgentBackend>>,
}

impl std::fmt::Debug for BackendRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BackendRegistry")
            .field("backend_ids", &self.backends.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl BackendRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, backend: Arc<dyn AgentBackend>) {
        let id = backend.id();
        tracing::debug!(id, "registering backend");
        self.backends.insert(id, backend);
    }

    /// Builder-style registration.
    pub fn with(mut self, backend: Arc<dyn AgentBackend>) -> Self {
        self.register(backend);
        self
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn AgentBackend>, SchedulerError> {
        self.backends.get(id).cloned().ok_or_else(|| {
            tracing::error!(id, "backend not found in registry");
            SchedulerError::UnknownBackend(id.to_owned())
        })
    }

    /// First registered backend (v0.1 single-backend default routing).
    pub fn default_backend(&self) -> Result<Arc<dyn AgentBackend>, SchedulerError> {
        self.backends.values().next().cloned().ok_or_else(|| {
            tracing::error!("no backend registered");
            SchedulerError::NoBackendRegistered
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::*;
    use async_trait::async_trait;

    /// Minimal stub backend for registry tests.
    struct TestBackend {
        id: &'static str,
    }

    #[async_trait]
    impl AgentBackend for TestBackend {
        fn id(&self) -> &'static str {
            self.id
        }
        fn capabilities(&self) -> AgentCapabilities {
            AgentCapabilities::default()
        }
        async fn run(
            &self,
            _task: AgentTask,
            _ctx: RunContext,
        ) -> Result<AgentResult, BackendError> {
            unimplemented!("registry tests never invoke run()")
        }
    }

    fn make_backend(id: &'static str) -> Arc<dyn AgentBackend> {
        Arc::new(TestBackend { id })
    }

    // ── construction ────────────────────────────────────────────

    #[test]
    fn test_new_is_empty() {
        let reg = BackendRegistry::new();
        assert!(matches!(
            reg.default_backend(),
            Err(SchedulerError::NoBackendRegistered)
        ));
    }

    #[test]
    fn test_default_is_empty() {
        let reg = BackendRegistry::default();
        assert!(matches!(
            reg.default_backend(),
            Err(SchedulerError::NoBackendRegistered)
        ));
    }

    // ── register / with ────────────────────────────────────────

    #[test]
    fn test_register_adds_backend() {
        let mut reg = BackendRegistry::new();
        reg.register(make_backend("alpha"));
        assert_eq!(reg.get("alpha").unwrap().id(), "alpha");
    }

    #[test]
    fn test_with_builder_pattern() {
        let reg = BackendRegistry::new()
            .with(make_backend("a"))
            .with(make_backend("b"))
            .with(make_backend("c"));
        assert_eq!(reg.get("a").unwrap().id(), "a");
        assert_eq!(reg.get("b").unwrap().id(), "b");
        assert_eq!(reg.get("c").unwrap().id(), "c");
    }

    #[test]
    fn test_register_overwrites_existing_id() {
        let mut reg = BackendRegistry::new();
        reg.register(make_backend("dup"));
        reg.register(make_backend("dup"));
        assert_eq!(reg.get("dup").unwrap().id(), "dup");
        // Still only one entry.
        assert!(reg.get("other").is_err());
    }

    // ── get ────────────────────────────────────────────────────

    #[test]
    fn test_get_unknown_backend() {
        let reg = BackendRegistry::new();
        assert!(matches!(
            reg.get("nonexistent"),
            Err(SchedulerError::UnknownBackend(_))
        ));
    }

    #[test]
    fn test_get_after_register() {
        let mut reg = BackendRegistry::new();
        reg.register(make_backend("x"));
        assert_eq!(reg.get("x").unwrap().id(), "x");
    }

    #[test]
    fn test_get_multiple_backends() {
        let mut reg = BackendRegistry::new();
        reg.register(make_backend("first"));
        reg.register(make_backend("second"));
        reg.register(make_backend("third"));
        assert_eq!(reg.get("first").unwrap().id(), "first");
        assert_eq!(reg.get("second").unwrap().id(), "second");
        assert_eq!(reg.get("third").unwrap().id(), "third");
        assert!(reg.get("fourth").is_err());
    }

    // ── default_backend ────────────────────────────────────────

    #[test]
    fn test_default_backend_empty() {
        let reg = BackendRegistry::new();
        assert!(matches!(
            reg.default_backend(),
            Err(SchedulerError::NoBackendRegistered)
        ));
    }

    #[test]
    fn test_default_backend_returns_one_of_registered() {
        let mut reg = BackendRegistry::new();
        reg.register(make_backend("alpha"));
        reg.register(make_backend("beta"));
        let id = reg.default_backend().unwrap().id();
        // HashMap order is non-deterministic; either is acceptable.
        assert!(id == "alpha" || id == "beta", "unexpected id: {id}");
    }

    #[test]
    fn test_default_backend_single_entry() {
        let mut reg = BackendRegistry::new();
        reg.register(make_backend("sole"));
        assert_eq!(reg.default_backend().unwrap().id(), "sole");
    }

    // ── Clone ──────────────────────────────────────────────────

    #[test]
    fn test_clone_empty() {
        let reg = BackendRegistry::new();
        let cloned = reg.clone();
        assert!(matches!(
            cloned.default_backend(),
            Err(SchedulerError::NoBackendRegistered)
        ));
    }

    #[test]
    fn test_clone_is_independent() {
        let mut reg = BackendRegistry::new();
        reg.register(make_backend("original"));
        let mut cloned = reg.clone();
        cloned.register(make_backend("new-only"));
        // Original must not see the new backend.
        assert!(reg.get("new-only").is_err());
        assert_eq!(cloned.get("new-only").unwrap().id(), "new-only");
        // Both still have "original".
        assert_eq!(reg.get("original").unwrap().id(), "original");
        assert_eq!(cloned.get("original").unwrap().id(), "original");
    }

    // ── Debug ──────────────────────────────────────────────────

    #[test]
    fn test_debug_empty() {
        let reg = BackendRegistry::new();
        let s = format!("{:?}", reg);
        assert!(s.starts_with("BackendRegistry"));
    }

    #[test]
    fn test_debug_with_backends() {
        let mut reg = BackendRegistry::new();
        reg.register(make_backend("dbg-a"));
        reg.register(make_backend("dbg-b"));
        let s = format!("{:?}", reg);
        assert!(s.starts_with("BackendRegistry"));
        assert!(s.contains("dbg-a") || s.contains("dbg-b"));
    }
}
