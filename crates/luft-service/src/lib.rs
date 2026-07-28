//! # luft-service
//!
//! **Presentation-free run lifecycle, query functions, and domain service.**
//!
//! The service layer sits between the transport facade (`luft-mcp`) and the
//! runtime / scheduler. It provides:
//!
//! - **WorkflowService**: typed domain API (`request` → `response`), no transport deps
//! - **Run preparation**: resolve script source (NL / workflow file / raw Lua),
//!   extract meta, assign run directories.
//! - **Execution**: build the sandboxed runtime and execute the script.
//! - **Query**: synchronous read-only operations for status, events, findings,
//!   reports, and logs.
//! - **Phases view**: build structured phase/agent trees for UI rendering.
//!
//! ## Modules
//!
//! | Module | Responsibility |
//! |--------|---------------|
//! | [`service`] | `WorkflowService` trait + impl — the typed domain API |
//! | [`request`] | Request structs (`#[derive(Deserialize, JsonSchema)]`) |
//! | [`response`] | Response structs (`#[derive(Serialize)]`) |
//! | [`error`] | `ServiceError` enum |
//! | [`run`] | Run lifecycle: validate, resolve, prepare, execute |
//! | [`query`] | Read-only queries: status, events, findings, report, cancel |
//! | [`phases`] | Phase tree builder for CLI / UI rendering |
//!
//! [`service`]: service
//! [`request`]: request
//! [`response`]: response
//! [`error`]: error
//! [`run`]: run
//! [`query`]: query
//! [`phases`]: phases

pub mod error;
pub mod json_to_lua;
pub mod params;
pub mod phases;
pub mod query;
pub mod request;
pub mod response;
pub mod run;
pub mod service;

pub use error::ServiceError;
pub use request::*;
pub use response::*;
pub use service::WorkflowService;
