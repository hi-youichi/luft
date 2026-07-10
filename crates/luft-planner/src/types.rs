//! `planner/types` — Core data structures for the NL → Lua planner.
//!
//! These types support the `meta + script` dual-field output model:
//! - `meta.phases` drives progress display and tracking
//! - `script` is the authoritative Lua orchestration code
//!
//! When `meta` and `script` disagree, `script` wins — `meta` is used only
//! as a preview; the actual execution follows the Lua code.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Agent-generated raw output structure (parsed from output_schema JSON).
///
/// Corresponds to the output_schema enforced on the planner agent's response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanOutput {
    pub meta: PlanMeta,
    pub script: String,
}

/// Declarative phase description for progress display and tracking.
///
/// Only describes the top-level phase structure. Does NOT represent the
/// full Lua DSL semantics — the `script` field is the authoritative source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanMeta {
    /// Ordered list of phases in the workflow.
    pub phases: Vec<PhaseMeta>,
    /// Planner's reasoning for this workflow design (optional).
    #[serde(default)]
    pub reasoning: String,
}

/// Single phase in a workflow plan.
///
/// `depends_on` uses 0-based indices into the `phases` array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseMeta {
    /// Display label, e.g. "discovery".
    pub label: String,
    /// One-line description, e.g. "扫描代码库中的函数定义".
    pub detail: String,
    /// Estimated number of agents spawned in this phase (for count display).
    #[serde(default)]
    pub agents: u32,
    /// Indices of phases this phase depends on (0-based).
    #[serde(default)]
    pub depends_on: Vec<u32>,
}

/// Internal workflow representation used by the runtime.
///
/// Built from `PlanOutput` by [`super::build_workflow`].
#[derive(Debug, Clone)]
pub struct PlannedWorkflow {
    /// Parsed phase metadata for progress display.
    pub phases: Vec<PhaseMeta>,
    /// Validated Lua orchestration script.
    pub script: String,
    /// Planner reasoning text.
    pub reasoning: String,
}

impl From<PlanOutput> for PlannedWorkflow {
    fn from(output: PlanOutput) -> Self {
        Self {
            phases: output.meta.phases,
            script: output.script,
            reasoning: output.meta.reasoning,
        }
    }
}

impl PlannedWorkflow {
    /// Build a workflow from a script alone (legacy text-extraction mode).
    ///
    /// Used when `use_structured_output = false` or extraction fails.
    pub fn from_script(script: String) -> Self {
        Self {
            phases: Vec::new(),
            script,
            reasoning: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Validation result
// ---------------------------------------------------------------------------

/// Outcome of multi-layer plan validation.
#[derive(Debug, Default)]
pub struct ValidationResult {
    /// Hard errors that cause rejection.
    pub errors: Vec<String>,
    /// Soft issues that produce warnings.
    pub warnings: Vec<String>,
}

impl ValidationResult {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a hard error.
    pub fn add_error(&mut self, msg: impl Into<String>) {
        self.errors.push(msg.into());
    }

    /// Add a soft warning.
    pub fn add_warning(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// Returns `true` if the plan has no hard errors.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Returns `true` if the plan has any warnings.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }
}

impl fmt::Display for ValidationResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.errors.is_empty() && self.warnings.is_empty() {
            return write!(f, "valid");
        }
        if !self.errors.is_empty() {
            write!(f, "errors: {}", self.errors.join("; "))?;
        }
        if !self.warnings.is_empty() {
            if !self.errors.is_empty() {
                write!(f, "; ")?;
            }
            write!(f, "warnings: {}", self.warnings.join("; "))?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Planning state (for streaming / event-driven progress)
// ---------------------------------------------------------------------------

/// Snapshot of the planner's current activity.
///
/// Used to drive progress display during the planning loop.
#[derive(Debug, Clone)]
pub enum PlanningState {
    /// Planner is thinking / building the prompt.
    Thinking,
    /// Waiting for agent response.
    Generating,
    /// Validating the generated script.
    Validating,
    /// Planning succeeded.
    Done(PlannedWorkflow),
    /// Planning failed with an error message.
    Error(String),
}

impl fmt::Display for PlanningState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanningState::Thinking => write!(f, "thinking"),
            PlanningState::Generating => write!(f, "generating"),
            PlanningState::Validating => write!(f, "validating"),
            PlanningState::Done(_) => write!(f, "done"),
            PlanningState::Error(e) => write!(f, "error: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plan_output_deserialization() {
        let json = r#"{
            "meta": {
                "phases": [
                    { "label": "discovery", "detail": "find files", "agents": 1, "depends_on": [] },
                    { "label": "analysis", "detail": "analyze", "agents": 3, "depends_on": [0] }
                ],
                "reasoning": "standard pipeline"
            },
            "script": "phase(\"discovery\", 0)\nreport({ok=true})"
        }"#;
        let plan: PlanOutput = serde_json::from_str(json).unwrap();
        assert_eq!(plan.meta.phases.len(), 2);
        assert_eq!(plan.meta.phases[1].depends_on, vec![0]);
        assert!(plan.script.contains("report("));
    }

    #[test]
    fn test_phase_meta_defaults() {
        // agents and depends_on should default to 0 / empty when missing
        let json = r#"{
            "meta": { "phases": [{ "label": "x", "detail": "y" }], "reasoning": "" },
            "script": "report({})"
        }"#;
        let plan: PlanOutput = serde_json::from_str(json).unwrap();
        let phase = &plan.meta.phases[0];
        assert_eq!(phase.agents, 0);
        assert!(phase.depends_on.is_empty());
    }

    #[test]
    fn test_planned_workflow_from_plan_output() {
        let output = PlanOutput {
            meta: PlanMeta {
                phases: vec![PhaseMeta {
                    label: "test".into(),
                    detail: "test phase".into(),
                    agents: 2,
                    depends_on: vec![],
                }],
                reasoning: "test reasoning".into(),
            },
            script: "report({})".into(),
        };
        let workflow = PlannedWorkflow::from(output);
        assert_eq!(workflow.phases.len(), 1);
        assert_eq!(workflow.script, "report({})");
        assert_eq!(workflow.reasoning, "test reasoning");
    }

    #[test]
    fn test_planned_workflow_from_script() {
        let workflow = PlannedWorkflow::from_script("report({ok=true})".into());
        assert!(workflow.phases.is_empty());
        assert_eq!(workflow.script, "report({ok=true})");
        assert!(workflow.reasoning.is_empty());
    }

    #[test]
    fn test_validation_result_valid() {
        let result = ValidationResult::new();
        assert!(result.is_valid());
        assert!(!result.has_warnings());
    }

    #[test]
    fn test_validation_result_errors() {
        let mut result = ValidationResult::new();
        result.add_error("lua syntax error");
        assert!(!result.is_valid());
        assert!(!result.has_warnings());
        assert!(result.errors.contains(&"lua syntax error".to_string()));
    }

    #[test]
    fn test_validation_result_warnings() {
        let mut result = ValidationResult::new();
        result.add_warning("meta phase 'x' not found in script");
        assert!(result.is_valid());
        assert!(result.has_warnings());
    }

    #[test]
    fn test_validation_result_display() {
        let mut result = ValidationResult::new();
        result.add_error("syntax error");
        result.add_warning("no phases");
        assert_eq!(result.to_string(), "errors: syntax error; warnings: no phases");
    }

    #[test]
    fn test_planning_state_display() {
        assert_eq!(PlanningState::Thinking.to_string(), "thinking");
        assert_eq!(PlanningState::Generating.to_string(), "generating");
        assert_eq!(PlanningState::Validating.to_string(), "validating");
        assert_eq!(PlanningState::Done(PlannedWorkflow::from_script("".into())).to_string(), "done");
        assert_eq!(
            PlanningState::Error("boom".into()).to_string(),
            "error: boom"
        );
    }

    #[test]
    fn test_planning_state_done_variant() {
        let workflow = PlannedWorkflow::from_script("test script".into());
        let state = PlanningState::Done(workflow.clone());
        assert_eq!(state.to_string(), "done");
    }

    #[test]
    fn test_validation_result_errors_and_warnings() {
        let mut result = ValidationResult::new();
        result.add_error("error 1");
        result.add_warning("warning 1");
        result.add_error("error 2");
        
        assert_eq!(result.errors.len(), 2);
        assert_eq!(result.warnings.len(), 1);
        assert!(!result.is_valid());
        assert!(result.has_warnings());
    }
}