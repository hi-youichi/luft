//! Test utilities for resume and integration testing.
//!
//! Provides instrumented backends that can simulate crashes, blocking,
//! and call recording — useful for testing crash-and-resume scenarios.

use crate::contract::backend::{
    AgentBackend, AgentCapabilities, AgentResult, AgentStatus, AgentTask, BackendError, LogRef,
    RunContext,
};
use crate::contract::ids::TokenUsage;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct CallRecord {
    pub seq: u64,
    pub agent_name: Option<String>,
    pub thread_id: Option<String>,
    pub prompt: String,
}

/// Backend that calls `std::process::exit(1)` after N calls.
pub struct CrashBackend {
    canned: Value,
    crash_after: u64,
    count: AtomicU64,
}

impl CrashBackend {
    pub fn new(canned: Value, crash_after: u64) -> Self {
        Self {
            canned,
            crash_after,
            count: AtomicU64::new(0),
        }
    }
}

#[async_trait::async_trait]
impl AgentBackend for CrashBackend {
    fn id(&self) -> &'static str {
        "crash"
    }
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            mcp_injection: false,
            structured_output: false,
            models: vec![],
        }
    }
    async fn run(&self, task: AgentTask, _ctx: RunContext) -> Result<AgentResult, BackendError> {
        let n = self.count.fetch_add(1, Ordering::SeqCst) + 1;
        eprintln!(
            "[crash-backend] agent #{n}: {}",
            task.name.as_deref().unwrap_or("?")
        );
        if n >= self.crash_after {
            eprintln!("[crash-backend] exiting after {n} calls");
            std::process::exit(1);
        }
        Ok(AgentResult {
            agent_id: task.agent_id,
            status: AgentStatus::Ok,
            output: self.canned.clone(),
            thread_id: task.thread_id.clone(),
            findings: vec![],
            tokens_used: TokenUsage::default(),
            artifacts: vec![],
            logs: LogRef::default(),
        })
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Backend that records all dispatched agent names.
#[derive(Clone)]
pub struct CountingBackend {
    canned: Value,
    calls: Arc<Mutex<Vec<String>>>,
}

impl CountingBackend {
    pub fn new(canned: Value) -> Self {
        Self {
            canned,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
    pub fn total_calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl AgentBackend for CountingBackend {
    fn id(&self) -> &'static str {
        "counting"
    }
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            mcp_injection: false,
            structured_output: false,
            models: vec![],
        }
    }
    async fn run(&self, task: AgentTask, _ctx: RunContext) -> Result<AgentResult, BackendError> {
        self.calls
            .lock()
            .unwrap()
            .push(task.name.clone().unwrap_or_default());
        Ok(AgentResult {
            agent_id: task.agent_id,
            status: AgentStatus::Ok,
            output: self.canned.clone(),
            thread_id: task.thread_id.clone(),
            findings: vec![],
            tokens_used: TokenUsage::default(),
            artifacts: vec![],
            logs: LogRef::default(),
        })
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Backend with shared state via Arc for resume tests.
#[derive(Clone)]
pub struct SharedBackend {
    canned: Value,
    call_count: Arc<AtomicU64>,
    pub block_on: Arc<Mutex<Option<u64>>>,
    pub fail_on: Arc<Mutex<Option<u64>>>,
    calls: Arc<Mutex<Vec<CallRecord>>>,
}

impl SharedBackend {
    pub fn new(canned: Value) -> Self {
        Self {
            canned,
            call_count: Arc::new(AtomicU64::new(0)),
            block_on: Arc::new(Mutex::new(None)),
            fail_on: Arc::new(Mutex::new(None)),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
    pub fn total_calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

#[async_trait::async_trait]
impl AgentBackend for SharedBackend {
    fn id(&self) -> &'static str {
        "shared"
    }
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            mcp_injection: false,
            structured_output: false,
            models: vec![],
        }
    }
    async fn run(&self, task: AgentTask, ctx: RunContext) -> Result<AgentResult, BackendError> {
        let seq = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
        self.calls.lock().unwrap().push(CallRecord {
            seq,
            agent_name: task.name.clone(),
            thread_id: task.thread_id.clone(),
            prompt: task.prompt.clone(),
        });
        if self
            .fail_on
            .lock()
            .unwrap()
            .map(|n| n == seq)
            .unwrap_or(false)
        {
            return Err(BackendError::Execution("simulated failure".into()));
        }
        if self
            .block_on
            .lock()
            .unwrap()
            .map(|n| n == seq)
            .unwrap_or(false)
        {
            ctx.cancel.cancelled().await;
            return Err(BackendError::Cancelled);
        }
        Ok(AgentResult {
            agent_id: task.agent_id,
            status: AgentStatus::Ok,
            output: self.canned.clone(),
            thread_id: task.thread_id.clone(),
            findings: vec![],
            tokens_used: TokenUsage::default(),
            artifacts: vec![],
            logs: LogRef::default(),
        })
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}



