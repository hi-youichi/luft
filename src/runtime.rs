//! `runtime` — mlua orchestration runtime (M2). See code-design §4.
//!
//! The runtime executes Lua orchestration scripts in a sandboxed mlua VM.
//! Scripts call SDK primitives (`agent`, `parallel`, `converge`, `report`) which
//! bridge to the scheduler. The sandbox blocks `io`/`os`/`fs`/`network`.

mod converge;
mod error;
mod pipeline;
mod sandbox;

pub use converge::{ConvergeConfig, ConvergeResult, RoundStats};
pub use error::{ExecLimits, ScriptError};
pub use pipeline::{PipelineConfig, PipelineError, PipelineExecutor, PipelineItem, PipelineItemResult, PipelineResult, PipelineStage, PipelineStats, StageResult, StageStatus};
pub use sandbox::{Runtime, validate_script};


/// Validate a script without executing it (syntax + forbidden globals).
pub fn validate(script: &str) -> Result<(), ScriptError> {
    validate_script(script)
}