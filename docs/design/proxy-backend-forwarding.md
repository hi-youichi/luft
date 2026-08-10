# 方案：Proxy → Daemon Backend 转发

> **状态**：提案
>
> **目标**：`luft mcp serve --backend <id>` 在 daemon 已运行的情况下，也能将 backend 参数传递到 daemon，作为该连接的默认 backend。

---

## 摘要

**问题**：`luft mcp serve --backend codex` 只在 daemon 未运行时生效（通过 `luft daemon start --backend codex` 传递）。若 daemon 已在运行，`--backend` 被静默丢弃——Proxy 连上已有 daemon 后，backend 仍是 daemon 启动时的默认值。

**根因**：Proxy 是纯字节转发层，`run_proxy(addr)` 只收一个地址字符串，不传任何元数据到 daemon。

**方案**：Proxy 连接 daemon 时，将 `--backend` 附加到 WebSocket URL 的 query string 中。Daemon 在 accept 时提取该参数，为每个连接创建带有 `default_backend` 的 `LuftMcpServer` 实例。

**优先级链**（`workflow_execute` 中）：

```
req.backend (显式 per-request)
  > default_backend (Proxy --backend, per-connection)     ← NEW
    > Registry 默认 (daemon 启动时)
```

---

## 1. 问题

### 1.1 场景

```
终端 A: luft daemon start          # daemon auto-detect，首个 backend 为默认
终端 B: luft mcp serve --backend codex
```

期望：终端 B 的 MCP 客户端使用 `codex` 作为默认 backend。

实际：daemon 已在运行，`--backend codex` 在 `autostart.rs` 处被丢弃，连接使用 daemon 的默认 backend（auto-detect 首个）。

### 1.2 当前调用链

```
luft mcp serve --backend codex
  │
  ├─ discover_or_autostart()
  │   ├─ daemon 已运行 → 返回 addr，backend 丢弃 ❌
  │   └─ daemon 未运行 → spawn luft daemon start（无 --backend）
  │
  └─ run_proxy("127.0.0.1:7878")  ← 只传地址，不传 backend
       │
       └─ connect_async("ws://127.0.0.1:7878/mcp")  ← 无 backend 信息
```

### 1.3 为什么 `workflow_execute` 的 `backend` 参数不够

`backend-per-request.md` 已实现了 `workflow_execute` 的 `backend` 显式参数。但这是 **JSON-RPC 请求级别** 的。Proxy 的 `--backend` 是 **连接级别** 的意图——用户在启动 Proxy 时已经明确指定了 backend，不应要求每个 `workflow_execute` 调用都重复传参。

### 1.4 `client_name` 推断已被否决

`backend-per-request.md` 曾尝试通过 MCP `initialize` 握手时的 `client_info.name` 自动推断 backend（如 `"codex"` → `"codex"`）。

**此方案是错误的**：`client_name` 是 MCP 客户端身份标识，与 ACP backend 之间没有可靠的映射关系。同一个 MCP 客户端可能使用不同的 ACP backend，反之亦然。

因此本文档同时规划了**删除 `client_name` 推断逻辑**（见 §4.7），将 backend 选择完全交给 Proxy 的 query param。

---

## 2. 方案

### 2.1 总览

```
luft mcp serve --backend codex
  │
  ├─ discover_or_autostart()                            ← 不传 backend
  │   ├─ daemon 已运行 → 返回 addr
  │   └─ daemon 未运行 → spawn luft daemon start（auto-detect）
  │
  └─ run_proxy("127.0.0.1:7878", Some("codex"))        ← 传递 backend
       │
       └─ connect_async("ws://127.0.0.1:7878/mcp?backend=codex")  ← URL query
            │
            ▼
       Daemon handle_connection
            │  提取 query param "backend=codex"
            ▼
       LuftMcpServer { default_backend: Some("codex") }
            │
            ▼
       workflow_execute 时:
         req.backend 为 None → 使用 self.default_backend → "codex"
```

> **Daemon 不再有 `--backend` 参数。** Daemon 只做 auto-detect + 注册，不设默认值偏好。Backend 选择完全由 Proxy 的 query param 决定。

### 2.2 传递机制：WebSocket URL Query Param

选择 URL query param 的理由：

| 方案 | 优点 | 缺点 |
|------|------|------|
| **URL query param** ✅ | 不改协议；Proxy 保持纯转发；daemon 自然获取 | 无 |
| MCP `initialize` 扩展 | 协议原生 | Proxy 需解析 JSON-RPC，破坏纯代理模型 |
| 自定义 HTTP header | 标准做法 | `tokio_tungstenite::accept_hdr_async` 同样需要改 |
| 新 WS 端点 | 语义清晰 | 路由复杂度增加，收益不大 |

Query param 是最简方案：Proxy 改一行 URL 拼接，daemon 在 `accept_hdr_async` 回调中提取。

### 2.3 优先级链

在 `workflow_execute` 中，fallback 顺序调整为：

```
1. req.backend              — JSON-RPC 显式传入（per-request）
2. self.default_backend     — Proxy --backend 传递（per-connection）    ← NEW
3. Registry 默认             — daemon 启动时注册的默认 backend
```

**设计原则**：只有三层——显式请求 > 连接级默认 > 进程级默认。不再有 `client_name` 自动推断。

### 2.4 多连接隔离

每个 WS 连接通过 `with_fresh_client_name_and_backend` 创建独立的 `LuftMcpServer`：

```
Daemon
  │
  ├─ WS conn 1 (from Proxy --backend codex)
  │   LuftMcpServer { default_backend: Some("codex") }
  │
  ├─ WS conn 2 (from Proxy --backend opencode)
  │   LuftMcpServer { default_backend: Some("opencode") }
  │
  └─ WS conn 3 (from Proxy, no --backend)
      LuftMcpServer { default_backend: None }
```

所有连接共享 `WorkflowServiceImpl`（`Arc`），但 backend 默认值 per-connection 隔离。

---

## 3. 涉及文件

| 文件 | 改动 | 量 |
|------|------|-----|
| `crates/luft-mcp/src/proxy.rs` | `run_proxy` 签名增加 `backend: Option<&str>`；URL 拼接 `?backend=...` | ~10 行 |
| `crates/luft-cli/src/commands/mcp_server.rs` | 不传 backend 给 `discover_or_autostart`，只传给 `run_proxy` | ~3 行 |
| `crates/luft-cli/src/commands/daemon.rs` | **移除 `--backend` flag**（DaemonSubcommand::Start、run_foreground、spawn_detached） | ~15 行 |
| `crates/luft-daemon/src/autostart.rs` | **移除 `backend` 参数**（discover_or_autostart 签名） | ~5 行 |
| `crates/luft-daemon/src/server.rs` | `accept_async` → `accept_hdr_async`；提取 query param；`Arc<LuftMcpServer>` | ~25 行 |
| `crates/luft-mcp/src/server_rmcp.rs` | 新增 `default_backend` + `with_fresh_client_name_and_backend`；删除 `client_name`、`is_codex`、`with_fresh_client_name`、`infer_backend_from_client_name`；调整 `workflow_execute` 优先级 | ~40 行 |
| `docs/design/backend-per-request.md` | 更新优先级链（增加 proxy backend 层） | ~5 行 |

无新增依赖。Backend ID 不含特殊字符，无需 URL decode。

---

## 4. 详细实现

### 4.1 `proxy.rs` — 签名 + URL 拼接

```rust
/// Connect to `ws://addr/mcp[?backend=...]` and proxy stdio ↔ WS.
pub async fn run_proxy(daemon_addr: &str, backend: Option<&str>) -> Result<()> {
    let url = match backend {
        Some(b) if !b.is_empty() => format!("ws://{daemon_addr}/mcp?backend={b}"),
        _ => format!("ws://{daemon_addr}/mcp"),
    };

    let mut reconnect_attempt = 0u32;
    let mut delay = Duration::from_millis(INITIAL_RECONNECT_DELAY_MS);

    loop {
        let ws = connect_with_retry(&url).await?;
        // ... 其余不变
    }
}
```

`connect_with_retry` 改为接收完整 URL（不再内部拼接）：

```rust
async fn connect_with_retry(url: &str) -> Result<WebSocketStream<...>> {
    let mut delay = Duration::from_millis(50);
    let mut last_error = None;

    for attempt in 0..12 {
        match connect_async(url).await {
            Ok((ws, _response)) => return Ok(ws),
            // ...
        }
    }
    // ...
}
```

### 4.2 `mcp_server.rs` — 传递 backend

```rust
pub async fn serve(args: McpServeArgs) -> Result<()> {
    let addr = luft_daemon::discover_or_autostart().await?;
    luft_mcp::proxy::run_proxy(&addr, args.backend.as_deref()).await?;
    Ok(())
}
```

`discover_or_autostart` 不再接受 backend 参数——daemon 只做 auto-detect。

### 4.3 `daemon.rs` — 移除 `--backend`

```rust
#[derive(Debug, Subcommand)]
pub enum DaemonSubcommand {
    Start {
        #[arg(long, default_value_t = luft_daemon::autostart::DEFAULT_PORT)]
        port: u16,
        // ⬇ REMOVED: backend: Option<String>,
        #[arg(long)]
        foreground: bool,
    },
    // ...
}
```

`run_foreground` 恢复为纯 auto-detect：

```rust
async fn run_foreground(port: u16) -> Result<()> {
    let mut ids = backend::detect_available_backends();
    if ids.is_empty() {
        ids.push("mock");
    }
    let mut reg = luft_core::scheduler::BackendRegistry::new();
    for id in &ids {
        match backend::create_backend(id, false, None) {
            Ok(b) => {
                println!("  registered backend: {id}");
                reg = reg.with(b);
            }
            Err(e) => eprintln!("  failed to create backend '{id}': {e}"),
        }
    }
    // 不再有 `if let Some(ref id) = default_backend` 分支
    println!("Daemon backends: {}", ids.join(", "));
    let luft = luft::Luft::builder().registry(reg).build()?;
    // ...
}
```

`spawn_detached` 不再传 `--backend`：

```rust
async fn spawn_detached(port: u16) -> Result<()> {
    // ...
    cmd.arg("daemon")
        .arg("start")
        .arg("--port")
        .arg(port.to_string())
        .arg("--foreground");
    // 不再有: cmd.arg("--backend").arg(id);
    // ...
}
```

### 4.4 `autostart.rs` — 移除 backend 参数

```rust
pub async fn discover_or_autostart() -> Result<String> {
    // 签名不再接受 Option<String>
    // ...
}
```

### 4.5 `server.rs` — 提取 query param

accept loop 改为传递 `Arc<LuftMcpServer>`：

```rust
let mcp_server = Arc::new(LuftMcpServer::new(luft));

loop {
    // ...
    let server = Arc::clone(&mcp_server);
    tokio::spawn(async move {
        if let Err(e) = handle_connection(stream, peer, server).await {
            warn!(%peer, error = %e, "connection ended with error");
        }
    });
}
```

`handle_connection` 用 `accept_hdr_async` 提取 backend：

```rust
async fn handle_connection(
    stream: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    mcp_server: Arc<LuftMcpServer>,
) -> Result<()> {
    let backend: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);

    let ws_stream = tokio_tungstenite::accept_hdr_async(stream, |req, res| {
        if let Some(query) = req.uri().query() {
            for pair in query.split('&') {
                let mut kv = pair.splitn(2, '=');
                if kv.next() == Some("backend") {
                    if let Some(val) = kv.next() {
                        if !val.is_empty() {
                            *backend.borrow_mut() = Some(val.to_string());
                        }
                    }
                }
            }
        }
        Ok(res)
    })
    .await?;

    let server = mcp_server.with_fresh_client_name_and_backend(backend.into_inner());
    info!(%peer, backend = ?server.default_backend, "connection routed to /mcp");
    luft_mcp::ws_transport::serve_ws(server, ws_stream).await?;
    Ok(())
}
```

### 4.6 `server_rmcp.rs` — per-connection default_backend + 删除 `client_name`

新增字段：

```rust
pub struct LuftMcpServer {
    pub service: Arc<WorkflowServiceImpl>,
    tool_router: ToolRouter<Self>,
    // REMOVED: client_name: Arc<OnceLock<String>>,
    default_backend: Option<String>,  // NEW
}
```

删除以下全部内容：

```rust
// REMOVED: 字段
client_name: Arc<OnceLock<String>>,

// REMOVED: 方法
pub fn client_name(&self) -> Option<&str> { ... }
pub fn is_codex(&self) -> bool { ... }
pub fn with_fresh_client_name(&self) -> Self { ... }

// REMOVED: initialize 中的 set
let _ = self.client_name.set(request.client_info.name.clone());

// REMOVED: 推断函数
fn infer_backend_from_client_name(name: &str) -> Option<String> { ... }
```

新增构造方法：

```rust
/// Create a clone with optional per-connection backend.
/// Used by the daemon accept loop for each WS connection.
pub fn with_fresh_client_name_and_backend(&self, default_backend: Option<String>) -> Self {
    Self {
        service: Arc::clone(&self.service),
        tool_router: self.tool_router.clone(),
        default_backend,
    }
}
```

`new()` 初始化：

```rust
pub fn new(luft: Luft) -> Self {
    // ...
    let mut s = Self {
        service,
        tool_router: ToolRouter::default(),
        default_backend: None,
    };
    // ...
}
```

**关键改动** — `workflow_execute` 优先级（删除 `client_name` 推断）：

```rust
async fn workflow_execute(
    &self,
    Parameters(mut req): Parameters<ExecuteWorkflowRequest>,
) -> Result<String, String> {
    if req.backend.is_none() {
        // 1. Proxy --backend (per-connection, explicit user intent)
        if let Some(ref b) = self.default_backend {
            req.backend = Some(b.clone());
        }
        // 2. Registry default (handled by service.rs)
    }
    // ... rest unchanged
}
```

### 4.7 `server_rmcp.rs` — 删除 `client_name` 推断逻辑

以下代码**全部删除**：

```rust
// REMOVED: infer_backend_from_client_name 函数
fn infer_backend_from_client_name(name: &str) -> Option<String> {
    match name.to_ascii_lowercase().as_str() {
        "codex" => Some("codex".into()),
        "opencode" => Some("opencode".into()),
        _ => None,
    }
}
```

`workflow_execute` 中删除 `client_name` 分支：

```diff
  if req.backend.is_none() {
      if let Some(ref b) = self.default_backend {
          req.backend = Some(b.clone());
      }
-     else if let Some(name) = self.client_name.get() {
-         req.backend = infer_backend_from_client_name(name);
-     }
  }
```

> `client_name` 字段、`is_codex()`、`with_fresh_client_name`、`initialize` 中的 `set` 全部删除。`is_codex()` 在生产代码中零使用，仅测试引用。

---

## 5. 两种场景的完整流程

### 场景 A：daemon 已在运行

```
luft mcp serve --backend codex
  → discover_or_autostart()
      → daemon 已运行，返回 "127.0.0.1:7878"
  → run_proxy("127.0.0.1:7878", Some("codex"))
  → connect_async("ws://127.0.0.1:7878/mcp?backend=codex")
  → daemon accept_hdr_async → 提取 backend=codex
  → LuftMcpServer { default_backend: Some("codex") }
  → workflow_execute 时 req.backend 为 None → 取 self.default_backend → "codex" ✅
```

### 场景 B：daemon 未运行，自动拉起

```
luft mcp serve --backend codex
  → discover_or_autostart()
      → daemon 未运行
      → spawn "luft daemon start --foreground"（无 --backend）
      → daemon auto-detect 所有可用 backend，首个为默认
  → run_proxy("127.0.0.1:7878", Some("codex"))
  → connect_async("ws://127.0.0.1:7878/mcp?backend=codex")
  → daemon accept_hdr_async → 提取 backend=codex
  → LuftMcpServer { default_backend: Some("codex") }
  → query param 覆盖 daemon 默认 → "codex" ✅
```

> **唯一机制**：backend 选择只通过 query param。Daemon 的 auto-detect 只是注册后端，不表达偏好。

---

## 6. 向后兼容

| 场景 | 行为 |
|------|------|
| `luft mcp serve`（无 `--backend`） | `run_proxy(addr, None)` → URL 无 query param → `default_backend` 为 `None` → 走 registry 默认 |
| `luft daemon start` | 不再接受 `--backend`，纯 auto-detect |
| `luft run`（CLI 直连 daemon） | 不受影响，CLI 不经过 Proxy |
| 旧版 Proxy + 新版 daemon | 旧 Proxy 不传 query param → `default_backend = None` → 行为等同升级前 |
| 新版 Proxy + 旧版 daemon | 新版 Proxy 传 `?backend=codex` → 旧 daemon 忽略 query param → 无负面影响 |
| `workflow_execute(backend="mock")` 显式传入 | 仍优先于所有默认值（不受影响） |
| `resume_from_id` + Proxy `--backend` | `resume` 忽略 backend（继承原 run 的 backend），不受影响 |

---

## 7. 与 `backend-per-request.md` 的关系

`backend-per-request.md` 实现了 `req.backend` 显式参数（保留）和 `client_name` 推断（**本文档删除**）。

本文档在此基础上：
- **增加** per-connection 的 backend 默认值（Proxy `--backend` 转发）
- **删除** `client_name` 自动推断（`infer_backend_from_client_name` 函数及 `workflow_execute` 中的 fallback）

合并后的优先级：

```
Lua agent({backend="..."})
  > req.backend (显式 per-request)
    > default_backend (Proxy --backend, per-connection)    ← 本文档
      > Registry 默认 (daemon 启动)
```

> `client_name` 不再参与 backend 选择。`backend-per-request.md` 需同步更新，删除 Phase 1 中的 `infer_backend_from_client_name` 实现和 Phase 1 测试计划。

---

## 8. 测试计划

### 8.1 单元测试

| # | 测试 | 文件 |
|---|------|------|
| U1 | `with_fresh_client_name_and_backend` 创建独立实例，`default_backend` 正确传递 | `server_rmcp.rs` test mod |
| U2 | `workflow_execute` 优先级：`req.backend` > `default_backend` > registry 默认 | 同上 |
| U3 | `workflow_execute` 无 `default_backend` 时走 registry 默认 | 同上 |

### 8.2 集成测试

| # | 场景 | 期望 |
|---|------|------|
| I1 | Proxy 传 `?backend=mock`，不传 `req.backend` | workflow 使用 mock backend |
| I2 | Proxy 传 `?backend=mock`，显式 `req.backend="codex"` | workflow 使用 codex backend（显式优先） |
| I3 | Proxy 不传 backend | 走 registry 默认 |
| I4 | Proxy 传 `?backend=unknown`，daemon 无此 backend | `workflow_execute` 返回错误，含 `available_ids` |

---

## 9. 备选方案（已否决）

### 9.1 在 Proxy 中解析 JSON-RPC 注入 backend

> Proxy 解析 `workflow_execute` 请求，自动填充 `backend` 字段。

否决：破坏 Proxy 的纯转发模型；增加 Proxy 复杂度；需要解析每一条 JSON-RPC 消息。

### 9.2 per-connection `Luft` 实例

> 每个连接创建独立的 `Luft`（通过 `with_default_backend`），而非共享 `WorkflowServiceImpl`。

否决：`Luft` 包含 `active_runs`（`Arc<Mutex<HashMap>>`），多实例会导致 cancel/list 等操作无法跨连接工作。

### 9.3 修改 `discover_or_autostart` 返回 backend

> 改为 `discover_or_autostart` 返回 `(addr, Option<String>)`，在 daemon 已运行时保留 backend。

否决：这样 Proxy 知道了 backend 但 daemon 不知道——问题没解决，只是把 backend 留在了 Proxy 侧。