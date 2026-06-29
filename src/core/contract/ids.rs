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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default() {
        let t = TokenUsage::default();
        assert_eq!(t.input, 0);
        assert_eq!(t.output, 0);
        assert_eq!(t.cache_read, 0);
        assert_eq!(t.cache_write, 0);
    }

    #[test]
    fn test_total_basic() {
        let t = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 5,
            cache_write: 3,
        };
        assert_eq!(t.total(), 30);
    }

    #[test]
    fn test_total_zero() {
        let t = TokenUsage::default();
        assert_eq!(t.total(), 0);
    }

    #[test]
    fn test_total_only_input() {
        let t = TokenUsage {
            input: 100,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        };
        assert_eq!(t.total(), 100);
    }

    #[test]
    fn test_total_only_output() {
        let t = TokenUsage {
            input: 0,
            output: 200,
            cache_read: 0,
            cache_write: 0,
        };
        assert_eq!(t.total(), 200);
    }

    #[test]
    fn test_add() {
        let a = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 5,
            cache_write: 3,
        };
        let b = TokenUsage {
            input: 3,
            output: 7,
            cache_read: 2,
            cache_write: 1,
        };
        let result = a + b;
        assert_eq!(result.input, 13);
        assert_eq!(result.output, 27);
        assert_eq!(result.cache_read, 7);
        assert_eq!(result.cache_write, 4);
    }

    #[test]
    fn test_add_zero() {
        let a = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 5,
            cache_write: 3,
        };
        let zero = TokenUsage::default();
        let result = a + zero;
        assert_eq!(result.input, 10);
        assert_eq!(result.output, 20);
        assert_eq!(result.cache_read, 5);
        assert_eq!(result.cache_write, 3);
    }

    #[test]
    fn test_add_large() {
        let a = TokenUsage {
            input: u64::MAX,
            output: 0,
            cache_read: 0,
            cache_write: 0,
        };
        let b = TokenUsage {
            input: 0,
            output: u64::MAX,
            cache_read: 0,
            cache_write: 0,
        };
        let result = a + b;
        assert_eq!(result.input, u64::MAX);
        assert_eq!(result.output, u64::MAX);
    }

    #[test]
    fn test_add_assign() {
        let mut a = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 5,
            cache_write: 3,
        };
        let b = TokenUsage {
            input: 3,
            output: 7,
            cache_read: 2,
            cache_write: 1,
        };
        a += b;
        assert_eq!(a.input, 13);
        assert_eq!(a.output, 27);
        assert_eq!(a.cache_read, 7);
        assert_eq!(a.cache_write, 4);
    }

    #[test]
    fn test_add_assign_zero() {
        let mut a = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 5,
            cache_write: 3,
        };
        a += TokenUsage::default();
        assert_eq!(a.input, 10);
        assert_eq!(a.output, 20);
        assert_eq!(a.cache_read, 5);
        assert_eq!(a.cache_write, 3);
    }

    #[test]
    fn test_add_assign_chained() {
        let mut a = TokenUsage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
        };
        let b = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 30,
            cache_write: 40,
        };
        let c = TokenUsage {
            input: 100,
            output: 200,
            cache_read: 300,
            cache_write: 400,
        };
        a += b;
        a += c;
        assert_eq!(a.input, 111);
        assert_eq!(a.output, 222);
        assert_eq!(a.cache_read, 333);
        assert_eq!(a.cache_write, 444);
    }

    #[test]
    fn test_serialize_roundtrip() {
        let t = TokenUsage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
        };
        let json = serde_json::to_string(&t).unwrap();
        let deserialized: TokenUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, t);
    }

    #[test]
    fn test_serialize_default() {
        let t = TokenUsage::default();
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(
            json,
            r#"{"input":0,"output":0,"cache_read":0,"cache_write":0}"#
        );
        let deserialized: TokenUsage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, t);
    }

    #[test]
    fn test_debug_format() {
        let t = TokenUsage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
        };
        let debug = format!("{:?}", t);
        assert!(debug.contains("input: 1"));
        assert!(debug.contains("output: 2"));
        assert!(debug.contains("cache_read: 3"));
        assert!(debug.contains("cache_write: 4"));
    }

    #[test]
    fn test_clone() {
        let t = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 5,
            cache_write: 3,
        };
        let cloned = t;
        assert_eq!(cloned, t);
    }

    #[test]
    fn test_copy() {
        let t = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 5,
            cache_write: 3,
        };
        let copied = t;
        let also_t = t; // should not move — Copy semantics
        assert_eq!(copied, also_t);
    }

    #[test]
    fn test_add_commutative() {
        let a = TokenUsage {
            input: 5,
            output: 10,
            cache_read: 2,
            cache_write: 1,
        };
        let b = TokenUsage {
            input: 3,
            output: 7,
            cache_read: 4,
            cache_write: 6,
        };
        assert_eq!(a + b, b + a);
    }

    #[test]
    fn test_total_excludes_cache() {
        let t = TokenUsage {
            input: 10,
            output: 20,
            cache_read: 100,
            cache_write: 200,
        };
        assert_eq!(t.total(), 30);
    }

    #[test]
    fn test_add_assign_identity() {
        let mut a = TokenUsage {
            input: 5,
            output: 5,
            cache_read: 5,
            cache_write: 5,
        };
        a += TokenUsage::default();
        assert_eq!(
            a,
            TokenUsage {
                input: 5,
                output: 5,
                cache_read: 5,
                cache_write: 5
            }
        );
    }

    #[test]
    fn test_type_aliases() {
        // Verify type aliases exist and can be constructed
        let _run_id = RunId::nil();
        let _agent_id = AgentId::nil();
        let _phase_id: PhaseId = 42;
    }
}
