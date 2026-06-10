//! `agent(opts)` and `parallel(items, mapFn)` — the core scheduling primitives.
//!
//! Both build [`AgentTask`](crate::core::contract::backend::AgentTask)s from Lua
//! opts tables and, when a [`JournalStore`](crate::core::journal::JournalStore)
//! is present, consult the journal for cached results (M1 resume) before
//! submitting to the scheduler, recording outputs back keyed by cache key.
//!
//! Each primitive has its own registrar ([`single`] / [`parallel`]); the shared
//! resume/record plumbing lives in [`journal`].

mod journal;
mod parallel;
mod single;

use crate::runtime::sdk::SdkContext;
use mlua::Lua;

/// Register `agent` and `parallel` as Lua globals.
pub(crate) fn register_agent_sdk(lua: &Lua, cx: &SdkContext) -> mlua::Result<()> {
    single::register(lua, cx)?;
    parallel::register(lua, cx)?;
    Ok(())
}
