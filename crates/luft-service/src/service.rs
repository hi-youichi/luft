//! WorkflowService trait — the typed domain API.
//!
//! The trait and all request/response/error types live here in `luft-service`.
//! The concrete `WorkflowServiceImpl` lives in `luft-mcp` (which depends on
//! both `luft` and `luft-service`), avoiding a circular dependency.

use crate::error::ServiceError;
use crate::request::*;
use crate::response::*;

pub trait WorkflowService: Send + Sync {
    fn execute_workflow(
        &self,
        req: ExecuteWorkflowRequest,
    ) -> impl std::future::Future<Output = Result<ExecuteWorkflowResponse, ServiceError>> + Send;

    fn list_files(&self) -> Result<Vec<WorkflowFile>, ServiceError>;

    fn list_runs(&self, req: ListRunsRequest) -> Result<ListRunsResponse, ServiceError>;

    fn get_run_status(&self, req: GetRunStatusRequest) -> Result<RunStatusResponse, ServiceError>;

    fn get_run_events(&self, req: GetRunEventsRequest) -> Result<RunEventsResponse, ServiceError>;

    fn cancel_run(&self, req: CancelRunRequest) -> Result<CancelRunResponse, ServiceError>;
}
