# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Crate-level documentation for all 7 crates with quick-start examples,
  architecture diagrams, and API reference links.
- `AgentBackend` trait implementation guide with a complete code example.
- `MaestroBuilder` and `Maestro` public API documentation with `# Errors` sections.
- `RunHandle` and `RunOutcome` documentation with `IntoFuture` usage example.
- `#[must_use]` attributes on all builder and constructor methods.
- `CONTRIBUTING.md` with development setup and PR checklist.

## [0.2.0] - 2025-07-XX

### Added

- SQLite persistence layer (`maestro-storage`) replacing JSONL event logs.
- NL → Lua planner (`maestro-planner`) with retry and self-correction.
- `converge()` primitive for multi-round agent consensus.
- `pipeline()` primitive for non-barrier streaming stages.
- `phase_begin()` / `phase_end()` structural spans for observability.
- Run resume from checkpoint with `Maestro::start_resume()`.
- `RunHandle` implementing `IntoFuture` for ergonomic `.await`.
- Structured findings collection (`Finding`, `Severity`, `Location`).
- Token usage tracking (`TokenUsage`) with display helpers.
- `AgentStatus::as_str()` — canonical snake_case string mapping for
  checkpoint persistence (fixes silent `Debug` formatting drift).

### Changed

- **BREAKING**: `AgentBackend::run()` signature now takes `RunContext` (was
  bare `CancellationToken` + `EventSender`).
- **BREAKING**: `MaestroError` is now `#[non_exhaustive]`.
- **BREAKING**: `BackendError` is now `#[non_exhaustive]`.
- `MaestroBuilder` is now the sole constructor for `Maestro` (direct
  `Maestro::new()` removed).
- Run directory layout standardized: `.maestro/runs/<run-id>/`.

### Fixed

- `AgentStatus::TimedOut` persisted as `"timedout"` (missing underscore)
  instead of `"timed_out"` — checkpoint deserialization would silently
  mismatch the storage writer's canonical mapping.

## [0.1.0] - 2025-06-XX

### Added

- Initial release with `maestro`, `maestro-core`, `maestro-runtime`,
  `maestro-adapters`, and `maestro-service` crates.
- `Maestro` facade with `run_script()`, `run_workflow()`, `run_nl()`.
- Sandboxed mlua VM with `agent()`, `parallel()`, `report()`, `log()`.
- OpenCode ACP backend adapter.
- Scheduler with concurrency control and retry policy.
- Journal-based checkpoint store for run resume.
- CLI binary (`maestro-cli`) with `run`, `list`, `status`, `resume` commands.
