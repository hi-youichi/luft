//! Run registry — tracks active runs with their event bus and cancellation handles.
//!
//! Uses `DashMap` for lock-free concurrent access. Only **active** runs are kept
//! in the registry; finished runs are removed and their state lives on disk.

use crate::core::contract::event::AgentEvent;
use crate::core::contract::ids::RunId;
use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// RunHandle
// ---------------------------------------------------------------------------

/// A handle to an active run, shared between the WS layer and the background task.
pub struct RunHandle {
    /// Broadcast sender — the same one threaded into the run's `RunContext`
    /// (via `service::run::prepare`), so subscribers receive its events.
    pub events: broadcast::Sender<AgentEvent>,
    /// Cancellation token — calling `cancel()` signals the run to stop.
    pub cancel: CancellationToken,
    /// The background task executing the run (`service::run::execute`).
    pub task: JoinHandle<()>,
}

// ---------------------------------------------------------------------------
// RunRegistry
// ---------------------------------------------------------------------------

/// Active run registry (DashMap, supports concurrent read/write).
#[derive(Clone, Default)]
pub struct RunRegistry(Arc<DashMap<RunId, RunHandle>>);

impl RunRegistry {
    /// Insert a new active run.
    pub fn insert(&self, id: RunId, handle: RunHandle) {
        self.0.insert(id, handle);
    }

    /// Subscribe to an active run's event stream.
    /// Returns `None` if the run is not in the registry (may be finished or never existed).
    pub fn subscribe(&self, id: &RunId) -> Option<broadcast::Receiver<AgentEvent>> {
        self.0.get(id).map(|h| h.events.subscribe())
    }

    /// Check if a run is currently active.
    pub fn contains(&self, id: &RunId) -> bool {
        self.0.contains_key(id)
    }

    /// Send a cancellation signal to a specific run.
    /// Returns `true` if the run was found and cancelled, `false` if not in registry.
    pub fn cancel(&self, id: &RunId) -> bool {
        if let Some(handle) = self.0.get(id) {
            handle.cancel.cancel();
            true
        } else {
            false
        }
    }

    /// Remove a run from the registry (called after the run finishes).
    pub fn remove(&self, id: &RunId) {
        self.0.remove(id);
    }

    /// Cancel all active runs (used during graceful shutdown).
    pub fn cancel_all(&self) {
        for entry in self.0.iter() {
            entry.value().cancel.cancel();
        }
    }

    /// Get the number of active runs.
    #[allow(unused)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Check if there are no active runs.
    #[allow(unused)]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::contract::event::AgentEvent;
    use crate::core::contract::ids::RunId;
    use chrono::Utc;

    fn make_handle() -> RunHandle {
        let (tx, _rx) = tokio::sync::broadcast::channel(16);
        let cancel = tokio_util::sync::CancellationToken::new();
        let task = tokio::spawn(async {});
        RunHandle {
            events: tx,
            cancel,
            task,
        }
    }

    #[tokio::test]
    async fn registry_default_is_empty() {
        let reg = RunRegistry::default();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[tokio::test]
    async fn registry_insert_and_contains() {
        let reg = RunRegistry::default();
        let id = RunId::now_v7();
        reg.insert(id, make_handle());
        assert!(reg.contains(&id));
        assert_eq!(reg.len(), 1);
    }

    #[tokio::test]
    async fn registry_subscribe_existing() {
        let reg = RunRegistry::default();
        let id = RunId::now_v7();
        let handle = make_handle();
        reg.insert(id, handle);
        let mut rx = reg.subscribe(&id).unwrap();
        let h = reg.0.get(&id).unwrap();
        h.events.send(AgentEvent::RunStarted {
            run_id: id,
            task: "test".into(),
            ts: Utc::now(),
        }).unwrap();
        drop(h);
        let evt = rx.try_recv().unwrap();
        assert!(matches!(evt, AgentEvent::RunStarted { .. }));
    }

    #[tokio::test]
    async fn registry_subscribe_missing_returns_none() {
        let reg = RunRegistry::default();
        assert!(reg.subscribe(&RunId::now_v7()).is_none());
    }

    #[tokio::test]
    async fn registry_cancel_existing() {
        let reg = RunRegistry::default();
        let id = RunId::now_v7();
        reg.insert(id, make_handle());
        assert!(reg.cancel(&id));
    }

    #[tokio::test]
    async fn registry_cancel_missing() {
        let reg = RunRegistry::default();
        assert!(!reg.cancel(&RunId::now_v7()));
    }

    #[tokio::test]
    async fn registry_cancel_all() {
        let reg = RunRegistry::default();
        reg.insert(RunId::now_v7(), make_handle());
        reg.insert(RunId::now_v7(), make_handle());
        reg.cancel_all();
        assert_eq!(reg.len(), 2);
    }

    #[tokio::test]
    async fn registry_remove() {
        let reg = RunRegistry::default();
        let id = RunId::now_v7();
        reg.insert(id, make_handle());
        assert!(reg.contains(&id));
        reg.remove(&id);
        assert!(!reg.contains(&id));
    }

    #[tokio::test]
    async fn registry_insert_overwrites() {
        let reg = RunRegistry::default();
        let id = RunId::now_v7();
        reg.insert(id, make_handle());
        reg.insert(id, make_handle());
        assert_eq!(reg.len(), 1);
    }
}
