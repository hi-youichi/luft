//! # maestro-service
//!
//! **Presentation-free run lifecycle and query functions.**
//!
//! The service layer sits between the facade (`maestro`) and the runtime /
//! scheduler. It provides:
//!
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
//! | [`run`] | Run lifecycle: validate, resolve, prepare, execute |
//! | [`query`] | Read-only queries: status, events, findings, report, cancel |
//! | [`phases`] | Phase tree builder for CLI / UI rendering |
//!
//! [`run`]: run
//! [`query`]: query
//! [`phases`]: phases

pub mod phases;
pub mod query;
pub mod run;
