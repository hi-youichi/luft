//! # luft-service
//!
//! **Presentation-free run lifecycle and query functions.**
//!
//! The service layer sits between the facade (`luft`) and the runtime /
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

#[cfg(test)]
mod tests {
    #[test]
    fn submodules_are_accessible() {
        // Compile-time check: referencing each submodule's marker proves the
        // module path resolves. If any module becomes private / removed,
        // this file will fail to compile.
        let _: phases::__PhasesProbe = ();
        let _: query::__QueryProbe = ();
        let _: run::__RunProbe = ();
    }

    mod phases {
        #[cfg(test)]
        pub type __PhasesProbe = ();
    }
    mod query {
        #[cfg(test)]
        pub type __QueryProbe = ();
    }
    mod run {
        #[cfg(test)]
        pub type __RunProbe = ();
    }
}
