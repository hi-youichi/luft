//! Frozen contracts (code-design §1). These types are the shared basis for all
//! modules; once reviewed they should change rarely. A companion `CONTRACTS.md`
//! may track the freeze.

pub mod backend;
pub mod cache;
pub mod event;
pub mod finding;
pub mod ids;
pub mod schema;

pub use backend::*;
pub use cache::agent_cache_key;
pub use event::*;
pub use finding::*;
pub use ids::*;
pub use schema::*;
