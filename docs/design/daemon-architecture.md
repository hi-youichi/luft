# Daemon Architecture: Single-Process Workflow Execution

> **Status**: Design
> **Goal**: Centralize all workflow instances and tool execution in a single daemon process. Both `luft mcp serve` (MCP proxy) and `luft run` (CLI client) connect to the daemon over WebSocket. No `Luft` instance exists outside the daemon.

---

## 1. Background & Problem

### Current Architecture

```
Client (opencode / codex / ...)
  |  stdio (JSON-RPC)
  |
+--v--------------+
| luft mcp serve  |   Each process is independent
|  Arc<Luft>      |   Own tokio runtime
|  +- run task 1  |   Workflows run as tokio tasks in-process
|  +- run task 2  |   base_dir -> shared on disk
|  +- ...         |
+-----------------+
```

Each `luft mcp serve` invocation creates a fresh `Luft` instance with its own:
- `BackendRegistry` (backend connections, e.g. ACP subprocess handles)
- Tokio runtime (workflow tasks, `broadcast::Sender<AgentEvent>`, `CancellationToken`)
- In-memory `RunHandle` table (join handles, event subscribers)

**Run state is persisted to disk** (SQLite event log, checkpoint journal, run directory), but **live state is process-local**:
- `CancellationToken` - only the process that started a run can cancel it
- `broadcast::Sender<AgentEvent>` - only the process that started a run can subscribe to real-time events
- `JoinHandle` - only the process that started a run can await its completion

### The Problem

When multiple MCP server processes are running (multiple editor instances, multiple terminal sessions), each has its own isolated set of active workflows. A workflow started by Process A is invisible to Process B:

| Operation | Process A (started the run) | Process B (different process) |
|-----------|---------------------------|-------------------------------|
| `get_run_status` | Returns `running` (in-memory join handle alive) | Returns `running` (reads disk, but can't confirm liveness) |
| `cancel_run` | Signals `CancellationToken` -> run stops | Writes a cancel flag to disk; run may not notice |
| `get_run_events` | Real-time via `broadcast::Receiver` | Stale - reads from SQLite, no new events after process death |
| `execute_workflow` (resume) | Can resume (has the `RunHandle`) | Can resume from disk checkpoint, but loses in-flight state |

There is no mechanism for Process B to interact with workflows owned by Process A.

---

## 2. Goals

1. **Single process owns all workflows** - every `execute_workflow`, `cancel_run`, `luft run`, and event subscription hits the same `Arc<Luft>` instance.
2. **No `Luft` outside the daemon** - both `luft mcp serve` (stdio-to-WS proxy) and `luft run` (WS client with event streaming) are thin clients.
3. **Explicit daemon** - `luft daemon` starts the daemon; all other commands connect to it.
4. **Auto-start** - if a client cannot connect to a running daemon, it spawns one automatically.
5. **Crash recovery** - on daemon crash, clients auto-reconnect and re-spawn the daemon; in-flight workflows are recoverable via `resume_from_id`.
6. **Daemon owns one backend** - the daemon is started with a single `--backend <id>` (auto-detected if omitted). All workflows through that daemon share it. Need a different backend? Run a second daemon on another port.

### Non-Goals

- Horizontal scaling / multi-host clustering (single localhost daemon is sufficient)
- Daemon process supervision by an external watchdog (auto-start by secondary is enough)
- Backward compatibility with the old "each process is independent" mode (the daemon is now mandatory)

---

## 3. Architecture Overview

```
+----------------------------------------------------------+
|                    Daemon Process                          |
|  +------------+    +----------------------------------+   |
|  | Arc<Luft>  |    |     WebSocket Server              |   |
|  | (all runs) |<-->|     /mcp  -> MCP JSON-RPC          |   |
|  |            |    |     /run  -> Luft run protocol     |   |
|  +------------+    +----------------------------------+   |
|  +-----------------------------------------+             |
|  |  PID file: ~/.luft/daemon.pid            |             |
|  |  { pid: 12345, addr: "127.0.0.1:7878" }  |             |
|  +-----------------------------------------+             |
+-------+-------------------+------------------+------------+
        | WS /mcp           | WS /run          | WS /mcp
   +----v----------+   +---v-----------+   +---v-----------+
   | luft mcp serve|   |  luft run     |   | luft mcp serve|
   | (stdio <-> WS)|   |  (WS client)  |   | (stdio <-> WS)|
   |  No Luft      |   |  No Luft      |   |  No Luft      |
   +---------------+   +---------------+   +---------------+
        ^                    ^                    ^
        | stdio              | terminal            | stdio
   +----+----+          +---+----+          +---+----+
   | Client  |          | User   |          | Client |
   | (editor)|          | (CLI)  |          |(editor)|
   +---------+          +--------+          +--------+
```

### Process Roles

| Role | Binary | Protocol | Lifecycle | Owns `Luft`? |
|------|--------|----------|-----------|-------------|
| **Daemon** | `luft daemon` | serves both | Long-lived; started explicitly or auto-spawned | Yes - single `Arc<Luft>` |
| **MCP proxy** | `luft mcp serve` | `/mcp` | Short-lived; one per MCP client | No - pure proxy |
| **Run client** | `luft run` | `/run` | Short-lived; one per invocation | No - WS client |

---

## 4. Process Model

### 4.1 Daemon Process

The daemon is a standalone process that:

1. **Binds a TCP listener** on `127.0.0.1:<port>` (default: `7878`, configurable via `--port` / `LUFT_DAEMON_PORT`).
2. **Constructs a `Luft` instance** with the backend specified by `--backend` (auto-detected if omitted). All workflows share this single backend.
3. **Writes a PID file** to `~/.luft/daemon.pid` containing `{ pid, addr, started_at }`.
4. **Serves two WebSocket protocols**:
   - `/mcp` - MCP JSON-RPC for MCP clients (via `luft mcp serve` proxy).
   - `/run` - Luft run protocol for `luft run` CLI (start workflow + stream events).
5. **Manages graceful shutdown** - on `SIGTERM` / `Ctrl+C`, stop accepting new connections, wait for active workflows to complete (with a configurable timeout), then exit.

### 4.2 MCP Proxy Process (`luft mcp serve`)

The MCP proxy is a **transparent stdio-to-WS forwarder**:

1. **Reads the PID file** to find the daemon address.
2. **Connects via WebSocket** to `ws://addr/mcp`.
3. **Forwards MCP JSON-RPC** bidirectionally: client stdio -> WS -> daemon, daemon -> WS -> client stdio.
4. **No `Luft` instance** - no backend, no tokio runtime for workflows, no `base_dir` access.

If the daemon is not reachable, the proxy **auto-starts it** (see S5.2).

### 4.3 Run Client Process (`luft run`)

The run client is a **WebSocket client with event streaming**:

1. **Reads the PID file** to find the daemon address.
2. **Connects via WebSocket** to `ws://addr/run`.
3. **Sends a run request** containing the script (or resume ID) and execution options (`no_acp_raw`, `args`).
4. **Receives real-time events** streamed from the daemon (same `AgentEvent` objects that the in-process `broadcast::Sender` would deliver).
5. **Prints events** using the existing renderers (pretty terminal output, JSONL `--headless` mode, event log file).
6. **Receives completion** notification with final status and report.
7. **No `Luft` instance** - the client only handles presentation (rendering, file output, user interaction).

If the daemon is not reachable, the client **auto-starts it** (see S5.2).

### 4.4 Why WebSocket?

- **Bidirectional** - MCP requires full-duplex; the run protocol needs server-push for event streaming.
- **MCP-over-WS is already a known pattern** - the MCP spec lists WebSocket as a supported transport.
- **Simple framing** - each WebSocket text frame is one JSON message; no need for content-length headers or newline delimiters.
- **Library support** - `tokio-tungstenite` is lightweight and well-maintained.

---

## 5. Daemon Discovery & Auto-Start

### 5.1 Discovery

```
Client startup (luft mcp serve / luft run):
  1. Read ~/.luft/daemon.pid -> { pid, addr }
  2. If file exists:
     a. Check if PID is alive (kill(pid, 0) on Unix; OpenProcess on Windows)
     b. If alive: try WS connect to addr
        - Success -> ready
        - Fail (timeout/refused) -> PID file is stale, fall through to 5.2
     c. If dead: delete stale PID file, fall through to 5.2
  3. If file does not exist: fall through to 5.2
```

### 5.2 Auto-Start

```
  4. Spawn daemon: `luft daemon --port <port>` as a detached child process
     - Unix:    Command::new(current_exe).arg("daemon")...
                .spawn()  (child inherits no stdio; daemon redirects to log file)
     - Windows: same, with CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS
  5. Poll WS connect with backoff (10ms, 20ms, 40ms, ... up to 5s total)
  6. If connected -> ready
     If timeout -> error: "failed to start daemon within 5s"
```

### 5.3 Race Condition: Multiple Clients Start Simultaneously

Two clients may both fail to connect and both spawn a daemon. This is safe because:

- The daemon **tries to bind** `127.0.0.1:<port>`.
- If the port is already bound (first daemon won), the second daemon **exits immediately** with a clear error.
- The second client's spawned daemon exits, but the client's **poll loop** will succeed in connecting to the first daemon.

The **port bind is the mutex**. No file locking needed.

### 5.4 PID File Format

```json
{
  "pid": 12345,
  "addr": "127.0.0.1:7878",
  "started_at": "2025-08-19T10:30:00Z",
  "version": "0.3.4"
}
```

Location: `~/.luft/daemon.pid` (or `$LUFT_HOME/daemon.pid` if `LUFT_HOME` is set).

---

## 6. Communication Protocol

### 6.1 Transport Layer

The daemon serves two WebSocket endpoints on the same TCP listener:

- **`/mcp`** - MCP JSON-RPC (for `luft mcp serve` proxy and any MCP client)
- **`/run`** - Luft run protocol (for `luft run` CLI)

The WebSocket upgrade path determines which protocol the connection uses. Both are JSON-over-WS text frames.

### 6.2 MCP Protocol (`/mcp`)

The MCP protocol itself does not change. The daemon serves the **same 6 tools** as the current `luft mcp serve`:

| Tool | Daemon executes |
|------|----------------|
| `execute_workflow` | `luft.start_script()` / `start_workflow()` / `start_resume()` -> spawns tokio task |
| `list_files` | Reads workflow directories from disk |
| `list_runs` | `luft.list()` -> reads `base_dir` |
| `get_run_status` | `luft.status()` -> reads disk + in-memory state |
| `get_run_events` | `luft.events()` -> reads SQLite + live broadcast |
| `cancel_run` | `luft.cancel()` -> signals `CancellationToken` |

The daemon's `LuftMcpServer` is the **exact same `ServerHandler` impl** as today. The only change is the transport: `stdio()` -> WebSocket.

### 6.3 Run Protocol (`/run`)

The run protocol is a simple JSON message protocol for `luft run`. It is **not** MCP - it's a purpose-built protocol that supports real-time event streaming.

#### Client -> Server messages

```json
{"type": "start", "script": "<lua source>", "options": {"no_acp_raw": false, "args": {}}}
```

```json
{"type": "start", "resume_from_id": "<run_id>", "options": {...}}
```

```json
{"type": "cancel"}
```

#### Server -> Client messages

```json
{"type": "started", "run_id": "20250819-103000-abc123"}
```

```json
{"type": "event", "event": {<serialized AgentEvent>}}
```

```json
{"type": "complete", "status": "completed", "report": {<run report summary>}}
```

```json
{"type": "error", "message": "..."}
```

#### Flow

```
Client                          Daemon
  |                               |
  |--- start {script, options} -->|
  |                               | luft.start_script() -> RunHandle
  |<--- started {run_id} ---------|
  |                               |
  |                               | (workflow runs, events emitted)
  |<--- event {event} ------------|
  |<--- event {event} ------------|
  |<--- event {event} ------------|
  |                               |
  |--- cancel (optional) -------->|  (on Ctrl+C)
  |                               | luft.cancel()
  |                               |
  |<--- complete {status} --------|
  |                               |
```

The daemon subscribes to the run's `broadcast::Receiver` and forwards each `AgentEvent` as an `event` message. The client receives these and feeds them to the existing renderers (pretty terminal, JSONL, event log).

### 6.4 Multiple Connections

The daemon accepts multiple simultaneous WebSocket connections (any mix of `/mcp` and `/run`). All connections share the same `Arc<Luft>`. This means:

- A workflow started by `luft run` is **immediately visible** to an MCP client via `get_run_status`.
- An MCP client can `cancel_run` a workflow that `luft run` started.
- Both see the same `list_runs` output.

### 6.5 Session Isolation

Each WebSocket connection gets its own session:
- `/mcp` connections: independent MCP `initialize` handshake, independent `client_name` capture.
- `/run` connections: independent run request, independent event stream.

The `Arc<Luft>` is shared, but per-connection state is isolated.

---

## 7. Crash Recovery

### 7.1 Daemon Crash

When the daemon crashes:
- All tokio tasks (active workflows) die immediately.
- Disk state (checkpoints, event logs, SQLite) persists intact.
- All WebSocket connections drop.
- The PID file becomes stale (points to a dead PID).

### 7.2 Client Behavior on Disconnect

```
On WS disconnect:
  1. Any pending tool call or run request (sent but no response received) -> return error to user:
     "daemon connection lost: <reason>"
  2. Mark the connection as dead.
  3. On the NEXT operation from the user:
     a. Attempt to reconnect (read PID file -> WS connect)
     b. If fail -> auto-start new daemon (S5.2)
     c. If success -> resend the operation
  4. User can use resume_from_id to recover interrupted workflows.
```

**Key decision**: the client does NOT retry the failed operation automatically. It returns an error and lets the user decide. This avoids surprising side effects (e.g. double-executing a workflow).

### 7.3 What Happens to Interrupted Workflows?

| State | On disk? | Recoverable? |
|-------|----------|-------------|
| Completed phases (journal cache) | Yes (checkpoint.json) | Yes - `resume_from_id` skips them |
| In-flight agent task (mid-LLM-call) | No | No - the agent must re-run from phase start |
| Event log up to crash point | Yes (SQLite) | Yes - visible in `get_run_events` |
| Final report | No (run didn't complete) | No |

After daemon restart, `list_runs` will show the interrupted run with status `running` (stale). The client should:
1. Call `get_run_status` to inspect the interrupted run.
2. Call `execute_workflow` with `resume_from_id` to continue from the last checkpoint.

### 7.4 Daemon Graceful Shutdown

`luft daemon stop` (or `SIGTERM`):
1. Stop accepting new WS connections.
2. Signal all active `CancellationToken`s - workflows get a chance to checkpoint.
3. Wait up to `--shutdown-timeout` (default: 30s) for workflows to drain.
4. Force-kill any remaining tasks.
5. Delete the PID file.
6. Exit.

---

## 8. Implementation Plan

### 8.1 New Crate: `luft-daemon`

A new workspace member that ties together the daemon process logic.

```
crates/luft-daemon/
+- Cargo.toml          # depends on: luft, luft-mcp, tokio-tungstenite, tokio
+- src/
    +- lib.rs          # pub API: start_daemon(), discover_or_autostart()
    +- process.rs      # daemon discovery: read/write PID file, liveness check
    +- autostart.rs    # spawn detached daemon child process
    +- server.rs       # WebSocket server: TCP accept loop, route /mcp vs /run
    +- mcp_session.rs  # per-connection MCP JSON-RPC over WS (reuses LuftMcpServer)
    +- run_session.rs  # per-connection run protocol: start workflow + stream events
```

**Responsibilities**:
- Bind TCP listener, accept WebSocket connections.
- Route by WS path: `/mcp` -> MCP session, `/run` -> run session.
- MCP session: create a `LuftMcpServer` clone, drive MCP JSON-RPC over WS frames.
- Run session: parse run request, call `luft.start_script()`, subscribe to events, stream them back.
- Write/clean PID file.
- Graceful shutdown.

### 8.2 New Module: `luft-mcp` WebSocket Transport

Currently `luft-mcp` only has `serve_rmcp()` which uses `rmcp::transport::stdio`. We need a WebSocket transport:

```rust
// crates/luft-mcp/src/ws_transport.rs (new)
pub async fn serve_ws(server: LuftMcpServer, ws_stream: WebSocketStream<TcpStream>) -> Result<()>;
```

This drives the same `ServerHandler` impl over a WebSocket stream instead of stdio. The `rmcp` SDK supports custom transports via `IntoTransport`, so this should be a thin adapter.

### 8.3 New Module: `luft-mcp` Proxy

```rust
// crates/luft-mcp/src/proxy.rs (new)
pub async fn run_proxy(daemon_addr: &str) -> Result<()>;
```

The proxy:
1. Connects to `ws://daemon_addr/mcp`.
2. Spawns two tasks: stdio->WS and WS->stdio.
3. Exits when either side closes.

### 8.4 New Module: `luft-daemon` Run Client

```rust
// crates/luft-daemon/src/run_client.rs (new)
pub async fn run_via_daemon(
    daemon_addr: &str,
    request: RunRequest,
    event_handler: impl FnMut(AgentEvent),
) -> Result<RunResult>;
```

The run client:
1. Connects to `ws://daemon_addr/run`.
2. Sends a `start` message with the script/resume ID and all CLI options.
3. Receives `event` messages and passes them to the event handler (which feeds the existing renderers).
4. Receives `complete` message and returns the final result.
5. On Ctrl+C, sends `cancel` message.

### 8.5 CLI Changes

#### New command: `luft daemon`

```
luft daemon                              Start the daemon (foreground, blocks)
luft daemon --backend opencode --port 7878   Specify backend + port
luft daemon stop                         Signal the running daemon to shut down
luft daemon status                       Check if daemon is running (prints addr, PID, uptime)
```

`--backend` is optional (auto-detected if omitted, same logic as current `detect_backend()`). The daemon creates **one** backend instance shared by all workflows. `mcp serve` and `run` no longer accept `--backend`.

Added to `Commands` enum in `main.rs`:

```rust
/// Start or manage the Luft daemon (workflow execution backend).
#[command(subcommand)]
Daemon(commands::daemon::DaemonSubcommand),
```

#### Modified command: `luft mcp serve`

The `serve` handler changes from "construct `Luft` + serve stdio" to "discover/connect to daemon + proxy stdio<->WS":

```rust
// Before (current):
pub async fn serve(args: McpServeArgs) -> Result<()> {
    let backend = create_backend(...)?;
    let luft = Luft::builder().backend_arc(backend).build()?;
    let server = LuftMcpServer::new(luft);
    luft_mcp::serve_rmcp(server).await?;
}

// After:
pub async fn serve(args: McpServeArgs) -> Result<()> {
    let addr = luft_daemon::discover_or_autostart().await?;
    luft_mcp::proxy::run_proxy(&addr).await?;
}
```

#### Modified command: `luft run`

The `run` handler changes from "construct `Luft` + start workflow + read events" to "discover/connect to daemon + send run request + stream events":

```rust
// Before (current):
pub async fn run(args: RunArgs) -> Result<()> {
    let backend = create_backend(args.backend)?;
    let luft = Luft::builder().backend_arc(backend).build()?;
    let handle = luft.start_script(&script, ...)?;
    // read events from handle.events_rx, render, wait for completion
}

// After:
pub async fn run(args: RunArgs) -> Result<()> {
    let addr = luft_daemon::discover_or_autostart().await?;
    let request = RunRequest { script, no_acp_raw: args.no_acp_raw, args: args.args_json };
    luft_daemon::run_client::run_via_daemon(&addr, request, |event| {
        renderer.render_event(event);
    }).await?;
}
```

**Client-side** (never sent to daemon): `--confirm`, `--log`, `--log-format`, `--output`, `--workflow`, `--resume`, NL prompt → Lua generation (via `luft-planner`).

**Sent to daemon** in the run request: `no_acp_raw`, `args_json`.

Backend and model are **not** per-request — they are fixed at daemon startup via `luft daemon --backend <id>`.

### 8.6 Dependency Graph

```
luft-daemon
  +- luft          (Luft builder, BackendRegistry, AgentEvent)
  +- luft-mcp      (LuftMcpServer, ws_transport)
  +- tokio-tungstenite  (WebSocket)

luft-mcp (new modules)
  +- rmcp          (existing)
  +- tokio-tungstenite  (new)
  +- tokio         (existing)

luft-cli
  +- luft-daemon   (new dependency for mcp_server.rs and run.rs)
```

### 8.7 Phased Implementation

**Phase 1: WebSocket transport for `luft-mcp`**
- Add `tokio-tungstenite` dependency to `luft-mcp`.
- Implement `ws_transport.rs`: adapt WS stream to rmcp's `IntoTransport` trait.
- Implement `proxy.rs`: stdio<->WS bidirectional forwarder.
- Test: daemon-mode `serve_ws` + `run_proxy` in-process (no child process spawning).

**Phase 2: `luft-daemon` crate (MCP only)**
- Implement `process.rs`: PID file read/write, liveness check.
- Implement `autostart.rs`: spawn detached `luft daemon` child.
- Implement `server.rs`: TCP accept loop, WS upgrade, route `/mcp`.
- Implement `mcp_session.rs`: per-connection MCP JSON-RPC over WS.
- Implement `lib.rs`: `start_daemon()` + `discover_or_autostart()`.

**Phase 3: Run protocol**
- Implement `run_session.rs`: parse run request, call `luft.start_script()`, subscribe to events, stream back.
- Implement `run_client.rs`: connect to `/run`, send request, receive events, return result.
- Route `/run` in `server.rs`.

**Phase 4: CLI integration**
- Add `Daemon` subcommand to `main.rs`.
- Implement `commands/daemon.rs`: `Start`, `Stop`, `Status` subcommands.
- Rewrite `commands/mcp_server.rs::serve()` to use `discover_or_autostart` + `run_proxy`.
- Rewrite `commands/run.rs::run()` to use `discover_or_autostart` + `run_via_daemon`.

**Phase 5: Crash recovery & edge cases**
- Implement WS disconnect detection in proxy and run client.
- Implement reconnect + re-autostart logic.
- Add `--shutdown-timeout` to `luft daemon`.
- Integration tests: daemon crash, client reconnect, resume_from_id.

---

## 9. Resolved Decisions

### 9.1 Single backend per daemon

The daemon is started with **one** `--backend <id>` (auto-detected if omitted). All workflows through that daemon share it.

- `execute_workflow` (MCP tool) and `luft run` do **not** accept a `backend` parameter.
- The current `BackendRegistry` already supports this: register one backend at startup, it becomes the default.
- Need a different backend? Run `luft daemon --backend <other> --port 7879`.

This avoids designing a `BackendFactory` trait — the existing `BackendRegistry` is sufficient.

### 9.2 `luft run` goes through daemon

`luft run` is a daemon client, not a standalone process. It connects to `/run` and streams events. This ensures:
- All workflows are visible to all clients (MCP and CLI).
- `cancel_run` from one client can stop a workflow started by another.
- Single source of truth for run state.

The daemon must be running for `luft run` to work. Auto-start handles this transparently.

### 9.3 Daemon logging

- Logs to `~/.luft/logs/daemon.log`.
- Log level from `LUFT_LOG` env var or `--log-level` flag.
- The daemon logs: startup, WS connections (connect/disconnect), workflow starts/completes, errors.

### 9.4 Client-side vs daemon-side responsibilities

| Responsibility | Where |
|---------------|-------|
| Script generation (NL prompt -> planner -> Lua) | Client (`luft run`) |
| Script confirmation (`--confirm`) | Client (`luft run`) |
| Event rendering (pretty, JSONL) | Client (`luft run`) |
| Event log file (`--log`) | Client (`luft run`) |
| Report file (`--output`) | Client (`luft run`) |
| Backend creation | Daemon (once at startup via `--backend`) |
| Workflow execution | Daemon |
| Event broadcasting | Daemon (forwards to all connected clients) |
| Cancellation | Daemon (signals `CancellationToken`) |
| Auto-fix loop (`--auto-fix`) | Daemon (part of workflow execution) |

### 9.5 Stale run cleanup on daemon startup

After a crash, `list_runs` shows interrupted runs as `running` (stale). On daemon startup, scan `base_dir` for runs with `status: "running"` that have no live task — mark them as `interrupted`. Users can then `resume_from_id` to recover.

### 9.6 Graceful shutdown: checkpoint scope

`CancellationToken` gives the in-flight phase a chance to finish the current LLM turn, but **mid-phase checkpoint is not guaranteed**. On resume:
- Completed phases: skipped (checkpoint exists).
- In-flight phase: restarts from scratch (not mid-agent).

### 9.7 Leverage `luft-service` crate

The daemon's `mcp_session.rs` and `run_session.rs` should delegate to `WorkflowService` (from `luft-service`) rather than calling `Luft` methods directly. This matches the existing layered architecture: transport → service → runtime.

---

## 10. Testing

### 10.1 Test Pyramid

```
                     ┌──────────┐
                     │ E2E (13) │  Real binary, process spawning, real WS
                     ├──────────┤
                     │ Integ(13)│  In-process WS, real Luft + MockBackend
                     ├──────────┤
                     │ Unit (8) │  PID file, WS routing, autostart race
                     └──────────┘
```

- **Unit**: pure logic, no network, no tokio runtime — fast feedback for edge cases.
- **Integration**: in-process daemon (ephemeral port), real WebSocket connections, `MockBackend` — covers protocol correctness and multi-client semantics.
- **E2E**: spawn the actual `luft` binary via `assert_cmd`, real process lifecycle — covers auto-start, PID discovery, crash recovery, signal handling.

### 10.2 Unit Tests

| Module | Test | Description |
|---|---|---|
| `process.rs` | `pid_file_write_read_roundtrip` | Write PID file, read it back, verify all fields (pid, addr, started_at, version) |
| `process.rs` | `pid_file_stale_detection` | Write PID file with a dead PID → `is_alive()` returns false |
| `process.rs` | `pid_file_missing` | No PID file exists → `discover()` returns `NotFound` |
| `process.rs` | `pid_file_corrupt_json` | Garbage content in PID file → graceful error, not panic |
| `process.rs` | `pid_file_concurrent_read` | Multiple threads read the same PID file simultaneously → no corruption |
| `server.rs` | `route_path_mcp` | WS upgrade to `/mcp` → routed to MCP session handler |
| `server.rs` | `route_path_run` | WS upgrade to `/run` → routed to run session handler |
| `server.rs` | `route_path_unknown` | WS upgrade to `/unknown` → rejected with 404 |

### 10.3 Integration Tests

**Harness**: bind `127.0.0.1:0` (ephemeral port), spawn `daemon::serve(luft, listener)` as a tokio task. Connect via `tokio_tungstenite::connect_async`. Use `MockBackend` for deterministic, instant results.

```rust
struct DaemonEnv {
    addr: String,
    _handle: JoinHandle<()>,
    _tmp: TempDir,
}

impl DaemonEnv {
    async fn start() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let luft = Luft::builder()
            .backend(MockBackend::new("mock", vec![]))
            .base_dir(tmp.path().join("runs"))
            .build().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let handle = tokio::spawn(daemon::serve(Arc::new(luft), listener));
        Self { addr, _handle: handle, _tmp: tmp }
    }

    async fn ws(&self, path: &str) -> WebSocketStream<TcpStream> {
        tokio_tungstenite::connect_async(format!("ws://{}{}", self.addr, path))
            .await.unwrap().0
    }
}
```

| ID | Test | Protocol | Description |
|---|---|---|---|
| IT-01 | `mcp_full_roundtrip` | `/mcp` | `initialize` → response; `tools/list` → 6 tools; `execute_workflow` with mock Lua → `run_id`; `get_run_status` → running → eventually completed |
| IT-02 | `mcp_list_files` | `/mcp` | Call `list_files` → returns `.lua` files from `base_dir/workflows/` |
| IT-03 | `mcp_list_runs` | `/mcp` | Start 3 workflows → `list_runs` returns all 3 with correct statuses |
| IT-04 | `mcp_get_run_events_paginated` | `/mcp` | Start multi-phase workflow → `get_run_events` with offset/limit → correct pagination, no gaps |
| IT-05 | `run_start_complete` | `/run` | Send `start` with mock Lua → receive `started` → receive `event` messages → receive `complete` with `completed` status |
| IT-06 | `run_cancel` | `/run` | Send `start` → `started` → send `cancel` → receive `complete` with `cancelled` status |
| IT-07 | `run_resume` | `/run` | Start workflow → kill WS before completion → new connection sends `start` with `resume_from_id` → completes from checkpoint |
| IT-08 | `run_event_order` | `/run` | Verify events arrive in order: `run_started` → `phase_started` → `agent_started` → `agent_done` → `phase_done` → `run_done` |
| IT-09 | `run_error_propagation` | `/run` | Send invalid Lua → receive `error` message with description, no `started`/`complete` |
| IT-10 | `multi_client_visibility` | `/mcp`+`/run` | Client A starts workflow via `/run`; Client B calls `get_run_status` via `/mcp` → sees the same run as `running` |
| IT-11 | `cross_client_cancel` | `/mcp`+`/run` | Client A starts via `/run`; Client B calls `cancel_run` via `/mcp` → Client A receives cancellation events |
| IT-12 | `disconnect_survives` | `/run` | Start workflow via `/run`, drop WS → workflow continues on daemon; reconnect via `/mcp` → `get_run_status` still `running` |
| IT-13 | `concurrent_runs` | `/run`×2 | Two `/run` connections start workflows simultaneously → both complete independently, events don't cross |

### 10.4 E2E Tests

**Harness**: use `assert_cmd` to spawn the compiled `luft` binary. Set `LUFT_HOME` to a `tempfile::TempDir` to isolate daemon state (PID file, logs, runs). Set `LUFT_DAEMON_PORT` to an ephemeral port to avoid conflicts.

```rust
struct E2eEnv {
    home: TempDir,
    port: u16,
    bin: PathBuf,
}

impl E2eEnv {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let port = pick_unused_port(); // e.g. `portpicker::pick_unused_port()`
        let bin = assert_cmd::cargo::main_binary_path().unwrap();
        Self { home, port, bin }
    }

    fn luft(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.env("LUFT_HOME", self.home.path())
           .env("LUFT_DAEMON_PORT", self.port.to_string());
        cmd.args(args);
        cmd
    }
}
```

| ID | Test | Duration | Description |
|---|---|---|---|
| E2E-01 | `daemon_start_stop` | ~3s | `luft daemon --backend mock` starts, binds port, writes PID file. `luft daemon stop` shuts down, removes PID file. |
| E2E-02 | `daemon_status` | ~3s | Start daemon → `luft daemon status` prints addr, PID, uptime. Stop → `luft daemon status` prints "not running". |
| E2E-03 | `autostart_on_mcp_serve` | ~5s | No daemon running → `luft mcp serve` → daemon auto-starts → send MCP `initialize` via stdin → success. |
| E2E-04 | `autostart_on_run` | ~5s | No daemon → `luft run --backend mock "simple task"` → daemon auto-starts → workflow completes → stdout has correct events. |
| E2E-05 | `mcp_serve_proxy_roundtrip` | ~5s | Daemon already running → `luft mcp serve` → send `initialize` + `tools/list` + `execute_workflow` via stdin → all proxied correctly, response on stdout. |
| E2E-06 | `run_pretty_output` | ~10s | `luft run --backend mock -w tests/fixtures/multi_phase.lua` → verify pretty terminal output shows phases and agents correctly. |
| E2E-07 | `run_headless_jsonl` | ~10s | `luft run --backend mock --headless -w tests/fixtures/multi_phase.lua` → verify JSONL output: each line is valid JSON with correct `type` fields. |
| E2E-08 | `daemon_crash_recovery` | ~10s | Start daemon → start long workflow via `luft run` (background) → kill daemon PID → workflow stops → `luft run --resume` → resumes from checkpoint. |
| E2E-09 | `pid_file_stale_recovery` | ~5s | Write stale PID file (dead PID + valid addr) → `luft daemon status` detects stale → `luft mcp serve` clears stale file and auto-starts new daemon. |
| E2E-10 | `concurrent_autostart` | ~5s | Two `luft mcp serve` invocations simultaneously, no daemon → one spawns daemon (wins port bind) → both connect successfully. |
| E2E-11 | `run_then_mcp_visibility` | ~10s | `luft run --backend mock -w slow.lua` starts (background) → while running, `luft mcp serve` → `list_runs` shows the active run → `get_run_status` returns `running`. |
| E2E-12 | `daemon_graceful_shutdown` | ~15s | Start daemon → start workflow via `luft run` → `luft daemon stop` → workflow gets cancellation signal → daemon exits after drain → PID file removed. |
| E2E-13 | `reconnect_after_disconnect` | ~10s | MCP client connected → kill WS connection → next MCP operation detects disconnect → reconnects (or re-autostarts) → operation succeeds. |

### 10.5 Test Fixtures

```
tests/fixtures/
  simple_ok.lua    -- 1 phase, 1 agent, instant success
  multi_phase.lua  -- 3 phases, 2 agents each, deterministic mock responses
  slow.lua         -- phases with sleep (for cancel/disconnect/graceful-shutdown tests)
  fail_phase.lua   -- agent that returns failure status
  invalid.lua      -- syntactically invalid Lua (for error propagation tests)
```

### 10.6 Priority

| Priority | Tests | Gate |
|---|---|---|
| **P0** (block merge) | IT-01, IT-05, IT-06, IT-10, E2E-01, E2E-04 | Core: MCP works, run works, shared state, daemon lifecycle |
| **P1** (first sprint) | IT-07, IT-08, IT-11, IT-12, IT-13, E2E-03, E2E-05, E2E-08 | Resume, cross-client, event ordering, disconnect, autostart, crash |
| **P2** (hardening) | All unit tests, IT-02, IT-03, IT-04, IT-09, E2E-02, E2E-06, E2E-07, E2E-09, E2E-10, E2E-11, E2E-12, E2E-13 | Edge cases, output formats, concurrency |

### 10.7 CI

- Unit + integration: every PR (fast, in-process).
- E2E: on push to `main` (slower, process spawning).
- All must pass on both Windows and Linux.
- E2E tests use generous timeouts (30s) and retry-once for known flaky scenarios (port bind races on Windows).

### 10.8 Assertion Guidelines

Assert against **serialized JSON**, not Rust enum variants — this catches serialization bugs:

```rust
// Good
let msg: serde_json::Value = serde_json::from_str(&raw).unwrap();
assert_eq!(msg["type"], "event");
assert_eq!(msg["event"]["type"], "run_started");
assert!(msg["event"]["run_id"].is_string());

// Bad — doesn't test the wire format
assert!(matches!(event, AgentEvent::RunStarted { .. }));
```
