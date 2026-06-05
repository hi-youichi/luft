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
