//! Basic ids and token accounting (§1.1).

use serde::{Deserialize, Serialize};

/// Run identifier — uuid v7 (time-ordered, sorts well on disk).
pub type RunId = uuid::Uuid;
/// Agent identifier — uuid v7.
pub type AgentId = uuid::Uuid;
/// Monotonic phase index (each top-level `parallel`/`converge` is one phase).
pub type PhaseId = u32;

/// Token usage, accumulated as a run progresses.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl std::ops::Add for TokenUsage {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self {
            input: self.input + o.input,
            output: self.output + o.output,
            cache_read: self.cache_read + o.cache_read,
            cache_write: self.cache_write + o.cache_write,
        }
    }
}

impl std::ops::AddAssign for TokenUsage {
    fn add_assign(&mut self, o: Self) {
        *self = *self + o;
    }
}

impl TokenUsage {
    /// Billable input + output (excludes cache counters).
    pub fn total(&self) -> u64 {
        self.input + self.output
    }
}
