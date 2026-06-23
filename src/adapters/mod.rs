//! `adapters` — OpenCode ACP backend (P0-A).
//!
//! [`AcpAdapter`] connects to an `opencode acp` subprocess as an ACP **client**
//! and implements [`AgentBackend`](crate::core::contract::backend::AgentBackend).
//! Submodules:
//! - [`acp_adapter`] — config + adapter + one-shot session lifecycle
//! - [`update_mapper`] — ACP `SessionUpdate` → Maestro `ProgressDelta`
//! - [`permission`] — non-interactive `request_permission` decisions
//! - [`result_collector`] — stop reason + message → `AgentResult`

mod acp_adapter;
mod permission;
mod result_collector;
mod update_mapper;

pub use acp_adapter::{AcpAdapter, AcpConfig};

use crate::core::BackendRegistry;
use std::sync::Arc;

/// Register an [`AcpAdapter`] (the `opencode` backend) with a registry.
pub fn register_acp_backend(registry: &mut BackendRegistry, config: AcpConfig) {
    registry.register(Arc::new(AcpAdapter::new(config)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::BackendRegistry;

    #[test]
    fn registers_backend_with_opencode_id() {
        let mut registry = BackendRegistry::new();
        register_acp_backend(&mut registry, AcpConfig::default());
        let backend = registry.get("opencode").unwrap();
        assert_eq!(backend.id(), "opencode");
    }

    #[test]
    fn backend_is_retrievable_after_register() {
        let mut registry = BackendRegistry::new();
        register_acp_backend(&mut registry, AcpConfig::default());
        assert!(registry.get("opencode").is_ok());
    }

    #[test]
    fn register_overwrites_existing() {
        let mut registry = BackendRegistry::new();
        register_acp_backend(&mut registry, AcpConfig::default());
        register_acp_backend(&mut registry, AcpConfig::default());
        let backend = registry.get("opencode").unwrap();
        assert_eq!(backend.id(), "opencode");
    }

    #[test]
    fn register_does_not_add_other_ids() {
        let mut registry = BackendRegistry::new();
        register_acp_backend(&mut registry, AcpConfig::default());
        assert!(registry.get("other").is_err());
    }
}
