//! `adapters` — OpenCode ACP backend (P0-A).
//!
//! [`AcpAdapter`] connects to an `opencode acp` subprocess as an ACP **client**
//! and implements [`AgentBackend`](maestro_core::contract::backend::AgentBackend).
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

use maestro_core::BackendRegistry;
use std::sync::Arc;

/// Register an [`AcpAdapter`] (the `opencode` backend) with a registry.
pub fn register_acp_backend(registry: &mut BackendRegistry, config: AcpConfig) {
    registry.register(Arc::new(AcpAdapter::new(config)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use maestro_core::BackendRegistry;

    #[test]
    fn registers_backend_with_opencode_id() {
        let mut registry = BackendRegistry::new();
        register_acp_backend(&mut registry, AcpConfig::default());
        let backend = registry.get("opencode").unwrap();
        assert_eq!(backend.id(), "opencode");
    }

    #[test]
    fn register_overwrites_existing() {
        let mut registry = BackendRegistry::new();
        // Two distinguishable configs: same `id` (so they collide on the
        // registry key) but different `model`. A correct overwrite replaces
        // the first registration entirely; a no-op-on-collision or append
        // implementation would keep the first `model`.
        let first = AcpConfig {
            model: Some("first".to_string()),
            ..AcpConfig::default()
        };
        let second = AcpConfig {
            model: Some("second".to_string()),
            ..AcpConfig::default()
        };
        register_acp_backend(&mut registry, first);
        register_acp_backend(&mut registry, second);
        let backend = registry.get("opencode").unwrap();
        assert_eq!(backend.id(), "opencode");
        // Downcast through `as_any` so we can read the per-adapter config
        // (not just `id`, which is identical for both registrations).
        let acp = backend
            .as_any()
            .downcast_ref::<AcpAdapter>()
            .expect("registered backend is an AcpAdapter");
        assert_eq!(acp.config().model.as_deref(), Some("second"));
    }

    #[test]
    fn register_does_not_add_other_ids() {
        let mut registry = BackendRegistry::new();
        register_acp_backend(&mut registry, AcpConfig::default());
        assert!(registry.get("other").is_err());
    }
}
