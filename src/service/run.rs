use crate::core::contract::ids::RunId;
use crate::core::state::{CheckpointStatus, RunCheckpoint};
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct RunInput {
    pub nl: Option<String>,
    pub workflow: Option<PathBuf>,
    pub script: Option<String>,
}

pub enum ValidateSourceError {
    NoneProvided,
    MultipleProvided,
}

pub fn validate_source(input: &RunInput) -> std::result::Result<(), ValidateSourceError> {
    let count = input.nl.is_some() as usize
        + input.workflow.is_some() as usize
        + input.script.is_some() as usize;
    match count {
        0 => Err(ValidateSourceError::NoneProvided),
        1 => Ok(()),
        _ => Err(ValidateSourceError::MultipleProvided),
    }
}

pub enum ScriptSource<'a> {
    Nl(&'a str),
    Workflow(&'a Path),
    Script(&'a str),
}

pub async fn resolve_script(
    source: ScriptSource<'_>,
    backend: Arc<dyn crate::core::contract::backend::AgentBackend>,
) -> Result<String> {
    match source {
        ScriptSource::Nl(nl) => {
            let cfg = crate::planner::PlannerConfig::default();
            let planned = crate::planner::plan_workflow(nl, backend, &cfg).await?;
            Ok(planned.script)
        }
        ScriptSource::Workflow(path) => {
            std::fs::read_to_string(path)
                .map_err(|e| anyhow::anyhow!("failed to read workflow file: {}", e))
        }
        ScriptSource::Script(s) => Ok(s.to_string()),
    }
}

pub enum ResumeCheck {
    CanResume,
    NotFound,
    NotResumable(CheckpointStatus),
}

pub fn check_resumable(run_id: RunId, base_dir: &Path) -> ResumeCheck {
    let run_dir = base_dir.join(run_id.to_string());
    if !run_dir.exists() {
        return ResumeCheck::NotFound;
    }

    let checkpoint_path = run_dir.join("checkpoint.json");
    if checkpoint_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&checkpoint_path) {
            if let Ok(cp) = serde_json::from_str::<RunCheckpoint>(&content) {
                if matches!(
                    cp.status,
                    CheckpointStatus::Completed | CheckpointStatus::Cancelled | CheckpointStatus::Failed
                ) {
                    return ResumeCheck::NotResumable(cp.status);
                }
            }
        }
    }

    ResumeCheck::CanResume
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn validate_source_none() {
        let input = RunInput { nl: None, workflow: None, script: None };
        assert!(matches!(
            validate_source(&input),
            Err(ValidateSourceError::NoneProvided)
        ));
    }

    #[test]
    fn validate_source_multiple() {
        let input = RunInput {
            nl: Some("hi".into()),
            workflow: Some(PathBuf::from("wf.lua")),
            script: None,
        };
        assert!(matches!(
            validate_source(&input),
            Err(ValidateSourceError::MultipleProvided)
        ));
    }

    #[test]
    fn validate_source_nl_only() {
        let input = RunInput { nl: Some("hi".into()), workflow: None, script: None };
        assert!(validate_source(&input).is_ok());
    }

    #[test]
    fn validate_source_workflow_only() {
        let input = RunInput { nl: None, workflow: Some(PathBuf::from("wf.lua")), script: None };
        assert!(validate_source(&input).is_ok());
    }

    #[test]
    fn validate_source_script_only() {
        let input = RunInput { nl: None, workflow: None, script: Some("print(1)".into()) };
        assert!(validate_source(&input).is_ok());
    }

    #[test]
    fn resume_check_not_found() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_notfound");
        let run_id = RunId::now_v7();
        let result = check_resumable(run_id, &temp_dir);
        assert!(matches!(result, ResumeCheck::NotFound));
    }

    #[test]
    fn resume_check_completed() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_completed");
        let run_id = RunId::now_v7();
        let run_dir = temp_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();

        let checkpoint = RunCheckpoint {
            run_id,
            task: "t".into(),
            status: CheckpointStatus::Completed,
            current_phase: 1,
            completed_phases: vec![],
            agent_results: HashMap::new(),
            findings: vec![],
            total_tokens: 0,
            created_at: 0,
            updated_at: 0,
        };
        std::fs::write(
            run_dir.join("checkpoint.json"),
            serde_json::to_string(&checkpoint).unwrap(),
        ).unwrap();

        let result = check_resumable(run_id, &temp_dir);
        assert!(matches!(result, ResumeCheck::NotResumable(CheckpointStatus::Completed)));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn resume_check_running() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_running");
        let run_id = RunId::now_v7();
        let run_dir = temp_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();

        let checkpoint = RunCheckpoint {
            run_id,
            task: "t".into(),
            status: CheckpointStatus::Running,
            current_phase: 1,
            completed_phases: vec![],
            agent_results: HashMap::new(),
            findings: vec![],
            total_tokens: 0,
            created_at: 0,
            updated_at: 0,
        };
        std::fs::write(
            run_dir.join("checkpoint.json"),
            serde_json::to_string(&checkpoint).unwrap(),
        ).unwrap();

        let result = check_resumable(run_id, &temp_dir);
        assert!(matches!(result, ResumeCheck::CanResume));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn resume_check_no_checkpoint() {
        let temp_dir = std::env::temp_dir().join("maestro_svc_test_resume_nockpt");
        let run_id = RunId::now_v7();
        let run_dir = temp_dir.join(run_id.to_string());
        std::fs::create_dir_all(&run_dir).unwrap();

        let result = check_resumable(run_id, &temp_dir);
        assert!(matches!(result, ResumeCheck::CanResume));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}