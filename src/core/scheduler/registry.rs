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
        self.backends.insert(backend.id(), backend);
    }

    /// Builder-style registration.
    pub fn with(mut self, backend: Arc<dyn AgentBackend>) -> Self {
        self.register(backend);
        self
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn AgentBackend>, SchedulerError> {
        self.backends
            .get(id)
            .cloned()
            .ok_or_else(|| SchedulerError::UnknownBackend(id.to_owned()))
    }

    /// First registered backend (v0.1 single-backend default routing).
    pub fn default_backend(&self) -> Result<Arc<dyn AgentBackend>, SchedulerError> {
        self.backends
            .values()
            .next()
            .cloned()
            .ok_or(SchedulerError::NoBackendRegistered)
    }
}
