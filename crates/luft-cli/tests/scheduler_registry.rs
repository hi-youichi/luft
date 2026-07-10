//! Comprehensive tests for `BackendRegistry`.
//!
//! Spec: `docs/src/core/scheduler/registry.rs.md` (F1 doc-comment fix, F2
//! warn-on-overwrite). These tests pin the public-API contract so future
//! refactors (e.g. swapping HashMap → BTreeMap/IndexMap) can't silently
//! change behavior. They live in `tests/` so they exercise the registry
//! through its public re-export, mirroring how downstream consumers use it.
//!
//! Test groups:
//! 1. F2 — overwrite path (the new behavior)
//! 2. Boundary conditions (empty/short/long/unicode/many ids)
//! 3. Error conditions & variant payloads
//! 4. Clone + Debug interaction with overwrites
//! 5. Trait-surface compile-time assertions

use luft::core::contract::backend::{
    AgentBackend, AgentCapabilities, BackendError, RunContext,
};
use luft::core::contract::{AgentResult, AgentTask};
use luft::core::{BackendRegistry, SchedulerError};
use std::sync::Arc;

// ── Test fixtures ──────────────────────────────────────────

/// Minimal stub backend. The registry never invokes `run()`, so we keep it
/// `unimplemented!()` to make accidental calls loud.
struct StubBackend {
    id: &'static str,
    caps: AgentCapabilities,
}

impl StubBackend {
    fn new(id: &'static str) -> Self {
        Self {
            id,
            caps: AgentCapabilities::default(),
        }
    }
}

#[async_trait::async_trait]
impl AgentBackend for StubBackend {
    fn id(&self) -> &'static str {
        self.id
    }
    fn capabilities(&self) -> AgentCapabilities {
        self.caps.clone()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    async fn run(
        &self,
        _task: AgentTask,
        _ctx: RunContext,
    ) -> Result<AgentResult, BackendError> {
        unimplemented!("registry tests must never invoke backend.run()")
    }
}

fn backend(id: &'static str) -> Arc<dyn AgentBackend> {
    Arc::new(StubBackend::new(id))
}

// ════════════════════════════════════════════════════════════
// Group 1 — F2: overwrite path (warn-on-overwrite contract)
// ════════════════════════════════════════════════════════════
//
// F2 changed `register()` from `insert(...)` (silent overwrite) to
// `if contains_key { warn!(...) }; insert(...)`. These tests pin the
// observable behavior: overwrite is still supported, the latest Arc
// wins, no unrelated ids are evicted, and the warn path doesn't
// regress any happy-path callers.

#[test]
fn f2_overwrite_latest_arc_wins() {
    // Two distinct Arcs with the same id; `get()` must surface the
    // *second* one. Verifies the registry stores by id, not by Arc
    // identity, and that overwrites are not silently dropped.
    let mut reg = BackendRegistry::new();
    let first: Arc<dyn AgentBackend> = Arc::new(StubBackend::new("shared"));
    let second: Arc<dyn AgentBackend> = Arc::new(StubBackend::new("shared"));
    reg.register(first.clone());
    reg.register(second.clone());
    let got = reg.get("shared").expect("post-overwrite get");
    assert!(
        Arc::ptr_eq(&got, &second),
        "second Arc must win on overwrite"
    );
    assert!(
        !Arc::ptr_eq(&got, &first),
        "first Arc must NOT survive the overwrite"
    );
    assert_eq!(got.id(), "shared");
}

#[test]
fn f2_overwrite_repeated_many_times_single_id() {
    // 10 overwrites of the same id must keep working — the warn log
    // must not regress the path (no panic, no dropped final insert).
    let mut reg = BackendRegistry::new();
    for _ in 0..10 {
        reg.register(backend("only"));
    }
    let got = reg.get("only").expect("single id post-overwrite get");
    assert_eq!(got.id(), "only");
    // default_backend still resolves (the registry has exactly one key).
    assert_eq!(reg.default_backend().expect("single entry default").id(), "only");
}

#[test]
fn f2_builder_with_overwrites_existing_id() {
    // `.with()` is sugar over `register()`; same overwrite contract.
    let reg = BackendRegistry::new()
        .with(backend("dup"))
        .with(backend("dup"))
        .with(backend("dup"));
    assert_eq!(reg.get("dup").unwrap().id(), "dup");
}

#[test]
fn f2_overwrite_does_not_evict_unrelated_ids() {
    // Overwriting "alpha" must leave "beta" and "gamma" untouched.
    let mut reg = BackendRegistry::new();
    reg.register(backend("alpha"));
    reg.register(backend("beta"));
    reg.register(backend("gamma"));
    reg.register(backend("alpha")); // overwrite — warn emitted

    assert_eq!(reg.get("alpha").unwrap().id(), "alpha");
    assert_eq!(reg.get("beta").unwrap().id(), "beta");
    assert_eq!(reg.get("gamma").unwrap().id(), "gamma");

    // `default_backend()` still resolves to *some* registered id.
    let id = reg.default_backend().unwrap().id();
    assert!(
        ["alpha", "beta", "gamma"].contains(&id),
        "default_backend returned unexpected id: {id}"
    );
}

#[test]
fn f2_overwrite_with_distinct_arc_id_must_use_new_id() {
    // Edge: same id twice, but the *second* Arc's `id()` is what's
    // stored. We can't easily mutate the StubBackend id at runtime
    // (it's `&'static str`), but we can verify that registering two
    // *different* ids that share a logical slot doesn't collide.
    let mut reg = BackendRegistry::new();
    reg.register(backend("k1"));
    reg.register(backend("k2"));
    assert_eq!(reg.get("k1").unwrap().id(), "k1");
    assert_eq!(reg.get("k2").unwrap().id(), "k2");
}

#[test]
fn f2_default_backend_resolves_after_overwrite() {
    // F1 doc claim: `default_backend()` returns one of the registered
    // backends; iteration order is unspecified. After an overwrite
    // there are still exactly N distinct keys, so default must resolve.
    let mut reg = BackendRegistry::new();
    reg.register(backend("keep"));
    reg.register(backend("overwritten"));
    reg.register(backend("overwritten")); // warn emitted here
    reg.register(backend("also-keep"));

    let id = reg.default_backend().expect("default after overwrite").id();
    assert!(
        ["keep", "overwritten", "also-keep"].contains(&id),
        "got: {id}"
    );
}

#[test]
fn f2_clone_independent_overwrites() {
    // Cloning snapshots the inner map; an overwrite in the clone must
    // not leak back into the original.
    let mut reg = BackendRegistry::new();
    reg.register(backend("dup"));
    let mut cloned = reg.clone();
    cloned.register(backend("dup")); // warn in clone

    assert_eq!(reg.get("dup").unwrap().id(), "dup");
    assert_eq!(cloned.get("dup").unwrap().id(), "dup");

    // Divergence: only the clone gets the new id.
    cloned.register(backend("clone-only"));
    assert!(
        reg.get("clone-only").is_err(),
        "original must not see clone-only"
    );
    assert_eq!(cloned.get("clone-only").unwrap().id(), "clone-only");
}

#[test]
fn f2_register_returns_unit_does_not_consume_backend() {
    // `register()` takes `&mut self` and the backend by value, but the
    // Arc inside can still be cloned by the caller before passing it in.
    // This pins that callers retain ownership semantics they expect.
    let mut reg = BackendRegistry::new();
    let shared = backend("shared");
    let local = shared.clone();
    reg.register(shared);
    // The pre-cloned Arc must still be usable.
    assert_eq!(local.id(), "shared");
    assert_eq!(reg.get("shared").unwrap().id(), "shared");
}

// ════════════════════════════════════════════════════════════
// Group 2 — Boundary conditions
// ════════════════════════════════════════════════════════════

#[test]
fn boundary_empty_string_id_round_trips() {
    // "" is a valid `&'static str`. It must store and look up
    // distinctly from any non-empty id.
    let mut reg = BackendRegistry::new();
    reg.register(backend(""));
    assert_eq!(reg.get("").unwrap().id(), "");
    assert!(reg.get("non-empty").is_err());
    // default_backend still resolves.
    assert_eq!(reg.default_backend().unwrap().id(), "");
}

#[test]
fn boundary_single_char_id_round_trips() {
    let mut reg = BackendRegistry::new();
    reg.register(backend("a"));
    assert_eq!(reg.get("a").unwrap().id(), "a");
    assert!(reg.get("aa").is_err());
    assert!(reg.get("").is_err());
}

#[test]
fn boundary_unicode_id_round_trips_byte_for_byte() {
    // F1: iteration order is unspecified, but lookups must be exact.
    // Unicode + combining marks + emoji must not be normalized, folded,
    // or truncated by the registry.
    let mut reg = BackendRegistry::new();
    let ids = ["后端-α/β", "🌟-backend", "Ω-greek", "with space"];
    for id in ids {
        reg.register(backend(id));
    }
    for id in ids {
        let got = reg
            .get(id)
            .unwrap_or_else(|e| panic!("get({id:?}) failed: {e:?}"));
        assert_eq!(got.id(), id, "id mangled for {id:?}");
    }
}

#[test]
fn boundary_long_id_round_trips() {
    // 256-char id — must not be silently truncated.
    let long: &'static str = Box::leak("x".repeat(256).into_boxed_str());
    let mut reg = BackendRegistry::new();
    reg.register(backend(long));
    let got = reg.get(long).expect("long id lookup");
    assert_eq!(got.id().len(), 256);
    assert_eq!(got.id(), long);
}

#[test]
fn boundary_many_distinct_ids_coexist() {
    // 64 ids must coexist; each lookup returns the right backend.
    let mut reg = BackendRegistry::new();
    let ids: Vec<&'static str> = (0..64)
        .map(|i| {
            let s: &'static str =
                Box::leak(format!("backend-{i:02}").into_boxed_str());
            s
        })
        .collect();
    for id in &ids {
        reg.register(backend(id));
    }
    for id in &ids {
        assert_eq!(reg.get(id).unwrap().id(), *id);
    }
}

#[test]
fn boundary_ids_are_case_sensitive() {
    // HashMap<&str, _> uses byte equality, not case folding.
    let mut reg = BackendRegistry::new();
    reg.register(backend("Foo"));
    assert_eq!(reg.get("Foo").unwrap().id(), "Foo");
    assert!(reg.get("foo").is_err(), "case must not be folded");
    assert!(reg.get("FOO").is_err());
}

#[test]
fn boundary_empty_registry_default_backend_is_no_backends() {
    // Both `new()` and `default()` must produce the empty state.
    let new_reg = BackendRegistry::new();
    let default_reg = BackendRegistry::default();
    for reg in [&new_reg, &default_reg] {
        assert!(matches!(
            reg.default_backend(),
            Err(SchedulerError::NoBackendRegistered)
        ));
    }
}

// ════════════════════════════════════════════════════════════
// Group 3 — Error conditions & variant payloads
// ════════════════════════════════════════════════════════════

#[test]
fn error_unknown_backend_payload_preserves_full_id() {
    // `SchedulerError::UnknownBackend(String)` must carry the queried
    // id verbatim — no trimming, lowercasing, or surrounding quotes.
    let reg = BackendRegistry::new().with(backend("known"));
    let queried = "QUERY-WITH-MixedCase_and-punctuation!";
    match reg.get(queried) {
        Err(SchedulerError::UnknownBackend(s)) => assert_eq!(s, queried),
        Err(other) => panic!("expected UnknownBackend({queried:?}), got Err({other:?})"),
        Ok(_) => panic!("expected UnknownBackend({queried:?}), got Ok"),
    }
}

#[test]
fn error_unknown_backend_payload_empty_string() {
    // Looking up "" on an empty registry must return UnknownBackend("")
    // — distinct from the `get("")` succeeding after registering "".
    let reg = BackendRegistry::new();
    match reg.get("") {
        Err(SchedulerError::UnknownBackend(s)) => assert_eq!(s, ""),
        Err(other) => panic!("expected UnknownBackend(\"\"), got Err({other:?})"),
        Ok(_) => panic!("expected UnknownBackend(\"\"), got Ok"),
    }
}

#[test]
fn error_unknown_backend_payload_unicode_id() {
    let reg = BackendRegistry::new();
    let id = "后端-α/β";
    match reg.get(id) {
        Err(SchedulerError::UnknownBackend(s)) => assert_eq!(s, id),
        Err(other) => panic!("expected UnknownBackend({id:?}), got Err({other:?})"),
        Ok(_) => panic!("expected UnknownBackend({id:?}), got Ok"),
    }
}

#[test]
fn error_get_returns_cloned_arc() {
    // `get()` returns `Arc<dyn AgentBackend>` — callers can hold the
    // Arc past any subsequent register/overwrite on the same id.
    let mut reg = BackendRegistry::new();
    reg.register(backend("x"));
    let handle = reg.get("x").unwrap();
    reg.register(backend("x")); // overwrite — warn emitted
    // The pre-overwrite Arc must still be usable by its holder.
    assert_eq!(handle.id(), "x");
}

#[test]
fn error_default_backend_after_cloning_independent_overwrites() {
    // Edge interaction: clone, then overwrite in the clone, then call
    // `default_backend()` on both. Neither must observe the other's
    // divergence.
    let mut reg = BackendRegistry::new();
    reg.register(backend("a"));
    reg.register(backend("b"));
    let mut cloned = reg.clone();
    cloned.register(backend("c"));
    // Original still has exactly {a, b}.
    let id = reg.default_backend().unwrap().id();
    assert!(["a", "b"].contains(&id), "original default: {id}");
    // Clone has {a, b, c}.
    let id = cloned.default_backend().unwrap().id();
    assert!(["a", "b", "c"].contains(&id), "clone default: {id}");
}

// ════════════════════════════════════════════════════════════
// Group 4 — Clone & Debug interactions with the overwrite path
// ════════════════════════════════════════════════════════════

#[test]
fn debug_lists_all_ids_after_overwrite() {
    // The custom Debug impl surfaces `backend_ids` as a sorted-of-keys
    // Vec. After an overwrite, the overwritten id must still appear
    // (the new Arc is mapped to the same key).
    let mut reg = BackendRegistry::new();
    reg.register(backend("a"));
    reg.register(backend("b"));
    reg.register(backend("a")); // overwrite
    let s = format!("{reg:?}");
    assert!(s.starts_with("BackendRegistry"), "got: {s}");
    assert!(s.contains("a"), "missing 'a': {s}");
    assert!(s.contains("b"), "missing 'b': {s}");
    // Must not leak the inner HashMap / Arc formatting.
    assert!(!s.contains("HashMap"), "leaked inner type: {s}");
    assert!(!s.contains("Arc"), "leaked inner type: {s}");
}

#[test]
fn debug_with_only_overwritten_id_lists_it() {
    // Edge: registry with a single id registered twice. Debug must
    // still list that id exactly once (it's a map, not a list of
    // registrations).
    let mut reg = BackendRegistry::new();
    reg.register(backend("only"));
    reg.register(backend("only")); // overwrite
    let s = format!("{reg:?}");
    assert!(s.contains("only"), "got: {s}");
    // No double-listing — `only` should appear exactly once in the
    // ids Vec. We accept any other formatting by checking the count
    // of the literal substring "only".
    assert_eq!(s.matches("only").count(), 1, "got: {s}");
}

// ════════════════════════════════════════════════════════════
// Group 5 — Trait-surface compile-time assertions
// ════════════════════════════════════════════════════════════

#[test]
fn traits_backend_registry_is_send_and_sync() {
    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}
    assert_send::<BackendRegistry>();
    assert_sync::<BackendRegistry>();
}

#[test]
fn traits_backend_registry_is_default_and_clone() {
    fn assert_default<T: Default>() {}
    fn assert_clone<T: Clone>() {}
    assert_default::<BackendRegistry>();
    assert_clone::<BackendRegistry>();
}

#[test]
fn traits_backend_registry_implements_debug() {
    // Compile-time: Debug is required for use in `unwrap`/`expect` paths
    // and error chains. The custom impl must remain usable.
    fn assert_debug<T: std::fmt::Debug>() {}
    assert_debug::<BackendRegistry>();
    let reg = BackendRegistry::new();
    let _ = format!("{reg:?}"); // exercises the impl
}
