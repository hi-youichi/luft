# Luft 可读性改进计划

> 基于 2025-08-19 工作树源码盘点 + adapters 异味审计 + 日志系统审计编写。
> 所有结论均附源码路径与行号；无源码支持的推测标注 **（待验证）**。

---

## 1. 背景、目标、非目标

### 1.1 背景

Luft 是基于 Lua 的多智能体编排运行时，当前有 **13 个 crate**、约 **50+ 源文件**、最大单文件 **1595 行**（`acp_adapter.rs`）。代码经过快速迭代，功能完整且可运行，但跨文件模式债务开始影响可读性和可维护性。

两份独立审查已识别出系统性问题：
- `docs/dev/adapters-smell-audit-report.json` — adapters 模块评分 6/10
- `docs/dev/logging-review.md` — 日志 run_id 覆盖率仅 48%

### 1.2 目标

- 提高新贡献者的 onboarding 速度：10 分钟内理解核心执行链路
- 消除跨文件类型安全丢失（StopReason → String、structured_output 硬编码）
- 将 unwrap/clone 密集区的错误处理对齐到项目既有 `thiserror` + `?` 模式
- 将日志 run_id 覆盖率从 48% 提升到 ≥90%
- 补齐 >200 行零测试文件的最低覆盖

### 1.3 非目标

- 不重写架构或改变 crate 间依赖方向
- 不引入新的运行时依赖（不替换 mlua/sqlx/tokio）
- 不改变公开 API 语义（`AgentBackend` trait 签名、`AgentEvent` 序列化格式）
- 不优化性能（本轮只关注可读性）

### 1.4 当前工作树说明

- **基准 commit**：当前 HEAD（2025-08-19）
- **11 个 workspace crate**：`luft`, `luft-core`, `luft-runtime`, `luft-adapters`, `luft-planner`, `luft-service`, `luft-storage`, `luft-cli`, `luft-daemon`, `luft-mcp`, `luft-skills`
- 当前 workspace 中没有 `workflow-adapters` 或 `workflow-cli`；旧审计中的这两个名称不属于当前基线。

---

## 2. 当前架构与核心执行链路

### 2.1 Crate 依赖图

```
                     ┌──────────────────────────────────────────────────┐
                     │              luft-cli (二进制入口)                │
                     │  main.rs → commands/* → backend.rs → AcpAdapter  │
                     └──────┬───────────────┬────────────────────────────┘
                            │               │
                 ┌──────────▼──┐    ┌───────▼────────┐
                 │  luft-daemon │    │   luft-mcp     │
                 │  (WS+HTTP)   │    │  (MCP/stdio)   │
                 └──────┬───────┘    └───────┬────────┘
                        │                    │
                 ┌──────▼────────────────────▼────────┐
                 │       luft-service (业务编排层)     │
                 │  WorkflowService trait + Impl       │
                 └──────┬──────────────────────────────┘
                        │
                 ┌──────▼──────────────────────────────┐
                 │          luft (facade crate)         │
                 │  LuftBuilder · Luft · RunHandle      │
                 │  run.rs: resolve → prepare → execute │
                 └──┬─────────┬──────────┬──────────────┘
                    │         │          │
          ┌─────────▼──┐  ┌──▼──────┐  ┌▼────────────┐
          │ luft-planner│  │luft-    │  │luft-storage │
          │ (NL→Lua)   │  │runtime  │  │(SQLite 7表) │
          └────────────┘  │(mlua VM)│  └─────────────┘
                          └──┬──────┘
                             │
          ┌──────────────────▼──────────────────┐
          │          luft-core (地基)            │
          │  contract/ · scheduler/ · journal    │
          │  state.rs · run_dir.rs · query.rs    │
          └──────────────────┬──────────────────┘
                             │
          ┌──────────────────▼──────────────────┐
          │       luft-adapters (ACP 后端)      │
          │  acp_adapter · permission            │
          │  result_collector · update_mapper    │
          └──────────────────────────────────────┘
```

### 2.2 核心执行链路（`luft run "<NL>"` 全路径）

```
CLI main.rs:24  Commands::Run
  └─ commands/run.rs  解析参数 → 调用 luft::run::resolve_fresh
       │
       ├─ run.rs:107  resolve_fresh(source, backend, planner_cfg)
       │    └─ planner: NL → Lua 脚本
       │
       ├─ run.rs:133  assign_dir_name(spec, base_dir)
       │    └─ core/run_dir: 生成 .luft/runs/<timestamp>-<slug>/
       │
       ├─ run.rs  prepare(spec) → 构造 Runtime + Journal
       │    └─ scheduler 初始化 → 注册 BackendRegistry
       │    └─ storage: open_db → SqliteCheckpointBackend
       │
       └─ run.rs:415  execute(run_ctx, runtime, script)
            └─ spawn_blocking → Runtime::execute(script)
                 │
                 └─ Lua VM 执行 SDK 原语:
                      ├─ agent()     → scheduler.run_agent() → AcpAdapter.run()
                      ├─ parallel()  → N × scheduler.run_agent() (并发)
                      ├─ pipeline()  → PipelineExecutor 多阶段
                      ├─ converge()  → ConvergeExecutor 对抗验证
                      └─ report()    → 返回最终结果
```

**关键文件行数**：

| 文件 | 行数 | unwrap | clone | 测试数 |
|------|------|--------|-------|--------|
| `luft-adapters/src/acp_adapter.rs` | 1595 | ~30 | ~15 | ~15 |
| `luft-cli/src/commands/artifact_writer.rs` | 1589 | 103 | 22 | ~10 |
| `luft-runtime/src/converge.rs` | 1448 | 101 | 25 | ~8 |
| `luft/src/run.rs` | current source | 106（全部位于 `#[cfg(test)]`） | 16 | 140 |
| `luft-storage/src/writer.rs` | 1272 | 65 | 3 | ~5 |
| `luft-core/src/contract/event.rs` | 1140 | 83 | 2 | ~12 |
| `luft-cli/src/commands/run.rs` | 1097 | 52 | 22 | ~3 |
| `luft-service/src/service.rs` | 1063 | — | — | ~5 |
| `luft-core/src/scheduler/mod.rs` | 1049 | — | — | ~15 |

---

## 3. 可读性问题总览表

| # | 优先级 | 问题 | 证据 | 影响 | 范围 |
|---|--------|------|------|------|------|
| P1 | **已解决** | StopReason 已由 ACP enum 映射为 Luft 内部 typed kind，状态判断改为显式 match | `acp_adapter.rs:99,1053-1090`; `result_collector.rs:14-124` | 新增 StopReason 变体需在映射处显式处理 | adapters |
| P2 | **已解决/文档过期** | 当前 `run.rs` 的 `.unwrap()`/`expect()` 仅位于 `#[cfg(test)] mod tests`；原审计把测试代码计入生产路径 | `luft/src/run.rs:494+` | 旧数量会夸大生产 panic 风险 | luft |
| P3 | **已解决** | Accumulator 3 个公共 Mutex 字段已收敛为内部状态，并通过语义化方法访问 | `update_mapper.rs:16-66` | 状态快照和更新入口集中管理 | adapters |
| P4 | **已解决** | `workflow_validate_schema` 工具名集中为 crate 内部常量，保留 exact 与 substring 的既有语义 | `lib.rs`; `permission.rs`; `update_mapper.rs` | 工具名只需在常量处维护 | adapters |
| P5 | **已解决/保留边界** | Runtime、Pipeline、Planner 与 Journal 持久化/GC 日志均已带 run_id；`flush()` 当前是无操作兼容方法，不产生日志 | `luft-runtime`; `luft-planner`; `luft-core/src/journal.rs` | 运行链路可串联，维护性边界已文档化 | 全局 |
| P6 | **已解决/文档过期** | 旧审计称 `connect_timeout` 未消费，但当前 initialize handshake 已使用 timeout | `acp_adapter.rs:690-721` | 旧结论会误导维护者 | adapters |
| P7 | **已解决** | 移除 `converge.rs` 全局 `#![allow(dead_code)]`，仅对当前未接入 sandbox registrar 的保留代码做精确标注 | `converge.rs:1,80-660` | 不再隐藏模块内其他死代码警告 | runtime |
| P8 | **已解决** | `luft/src/query.rs` 已增加空库、缺失 run、events/findings/report 错误路径测试 | `luft/src/query.rs:371-401` | 查询边界行为有最小回归保护 | luft |
| P9 | **已解决/文档过期** | CLI 主模块当前已有 326 项测试（325 passed, 1 ignored），原“0 测试”结论不再成立 | `luft-cli/src/main.rs:292+` | 原审计数字会误导维护者 | cli |
| P11 | **已解决** | `luft-core` 顶层文档已改为 dependency-aware foundation，避免错误暗示无依赖 | `luft-core/src/lib.rs:5-8` | 原文会误导贡献者 | core |
| P12 | **已解决** | `result_collector.rs:24` 的 `let output = if` 缩进已对齐 | `result_collector.rs:24` | 阅读障碍 | adapters |
| P13 | **已解决/不拆分** | `AgentEvent` 已增加 lifecycle/ACP/SDK/Pipeline 分区注释；保持单文件以维持序列化兼容 | `luft-core/src/contract/event.rs:16-245` | 导航改善，避免不必要的模块拆分 | core |
| P14 | **已解决** | 结构化结果工具使用集中常量，并明确记录其无条件放行是 workflow 结果提交所需的协议例外 | `permission.rs:39-42` | 安全语义可审计 | adapters |

---

## 4. 分主题详细方案

### 4.1 模块边界 / API

#### 4.1.1 方案：Accumulator 封装（P3）

**现状**：

```
// update_mapper.rs:16-26
pub struct Accumulator {
    pub message: Mutex<String>,              // 公共字段
    pub tokens: Mutex<TokenUsage>,           // 公共字段
    pub workflow_validate_schema: Mutex<Option<serde_json::Value>>,  // 公共字段
}
// 10 处调用方直接 acc.message.lock().unwrap()
```

**问题**：
- 3 个独立 Mutex 无封装，调用方各自 `.lock().unwrap()`
- 无 poisoning 恢复策略
- 锁粒度不可调整（无法合并为单个 Mutex）
- 字段名 `workflow_validate_schema` 泄露实现细节

**改进后**：

```rust
// update_mapper.rs (改进后)
pub struct Accumulator {
    inner: Mutex<AccumulatorInner>,
}

#[derive(Default)]
struct AccumulatorInner {
    message: String,
    tokens: TokenUsage,
    structured_output: Option<serde_json::Value>,
}

impl Accumulator {
    pub fn new() -> Self { Self::default() }
    pub fn append_message(&self, text: &str) { /* lock + push_str */ }
    pub fn add_tokens(&self, delta: TokenUsage) { /* lock + add */ }
    pub fn set_structured_output(&self, value: serde_json::Value) { /* lock + set */ }
    pub fn snapshot(&self) -> AccumulatorSnapshot { /* 一次性 lock + clone */ }
}

pub struct AccumulatorSnapshot {
    pub message: String,
    pub tokens: TokenUsage,
    pub structured_output: Option<serde_json::Value>,
}
```

**拟修改文件**：
- `crates/luft-adapters/src/update_mapper.rs` — 改结构 + 加方法
- `crates/luft-adapters/src/acp_adapter.rs` — 调用方适配（10 处）
- `crates/luft-adapters/src/result_collector.rs` — collect() 参数从 3 个字段改为 `AccumulatorSnapshot`

**行为不变量**：
- 消息累积顺序不变
- token 累加语义不变
- structured_output 最后写入者胜出语义不变

**风险**：低 — 纯封装重构，编译器保证正确性
**验证**：`cargo test -p luft-adapters` 全量通过

---

#### 4.1.2 旧审计项：空 crate（P10，已从当前 workspace 消失）

**现状**：

```
旧审计曾引用 `crates/workflow-adapters` 和 `crates/workflow-cli`，但当前
`Cargo.toml` 只有 11 个 workspace member，两个路径均不存在。
```

**结论**：无需修改；只需避免在新的审计和文档中继续引用这两个旧名称。

**验证**：`cargo metadata --no-deps` 显示当前 workspace member 列表。

---

### 4.2 控制流 / 错误处理

#### 4.2.1 StopReason 类型安全恢复（P1，已完成）

**历史问题**：

```
// 历史实现：acp_adapter.rs 曾将 ACP StopReason enum Debug-format 为 String
let stop_reason_str = format!("{:?}", stop_reason);  // "EndTurn" / "MaxTokens" / ...
state.stop_holder.lock().unwrap() = Some(stop_reason_str);

// 历史实现：result_collector.rs 曾使用子串匹配
fn status_from_stop_reason(s: &str) -> AgentStatus {
    if s.contains("EndTurn") { AgentStatus::Ok }
    else if s.contains("Cancel") { AgentStatus::Cancelled }
    else { AgentStatus::Error }  // ← 新增变体静默落到此处
}
```

**问题**：
- 类型安全从 enum 降级为 String 子串匹配
- ACP 协议新增 StopReason 变体时静默 fallthrough 为 Error
- `format!("{:?}", ...)` 依赖 Debug 实现的格式稳定性

**当前实现**：

```rust
// acp_adapter.rs：ACP enum 映射到 Luft 内部 StopReasonKind
stop_holder: Arc<Mutex<Option<StopReasonKind>>>

// result_collector.rs：显式 match，不再使用 contains()
match reason {
    StopReasonKind::EndTurn => AgentStatus::Ok,
    StopReasonKind::Cancelled => AgentStatus::Cancelled,
    StopReasonKind::MaxTokens
    | StopReasonKind::MaxTurnRequests
    | StopReasonKind::Refusal
    | StopReasonKind::Unknown => AgentStatus::Error,
}
```

**拟修改文件**：
- `crates/luft-adapters/src/acp_adapter.rs:99,1053-1090` — 停止 Debug-format，使用 typed mapping
- `crates/luft-adapters/src/result_collector.rs:14-124` — `StopReasonKind` 与显式状态匹配

**行为不变量**：
- EndTurn → Ok
- Cancel → Cancelled
- 其余 → Error
- 语义完全等价

**风险**：已验证 — 仅 typed kind 跨线程传递，不依赖 ACP macro-generated enum 的 `Send` 语义。
**验证**：`cargo test -p luft-adapters --lib`（89 passed）+ `cargo check -p luft-adapters -p luft-cli`。

---

#### 4.2.2 方案：structured_output 魔法字符串提取（P4）

**现状**：

```
// "structured_output" 出现在 5 处 3 文件中：
// permission.rs:39    — 策略绕过（exact match: tool == "workflow_validate_schema"）
// update_mapper.rs    — 捕获触发（substring match on SessionUpdate JSON）
// acp_adapter.rs      — capability flag（config.emit_raw_events 相关）
// result_collector.rs — 输出优先级（has_structured 判断）
```

审计报告原文："inconsistent exact-vs-substring matching that is a latent bug"

**改进后**：

```rust
// 新增 luft-adapters/src/constants.rs
/// MCP tool name for structured output validation.
pub const STRUCTURED_OUTPUT_TOOL: &str = "workflow_validate_schema";

/// JSON key used in ACP session updates to signal structured output.
pub const STRUCTURED_OUTPUT_KEY: &str = "structured_output";
```

- 所有 5 处引用统一使用常量
- 所有匹配统一为 exact match（除非有明确理由用 substring）
- 在 `permission.rs:39` 添加注释说明为何无条件放行

**拟修改文件**：
- `crates/luft-adapters/src/constants.rs` — 新建
- `crates/luft-adapters/src/lib.rs` — mod constants
- `crates/luft-adapters/src/permission.rs:38-41`
- `crates/luft-adapters/src/update_mapper.rs`
- `crates/luft-adapters/src/acp_adapter.rs`
- `crates/luft-adapters/src/result_collector.rs`

**行为不变量**：
- 放行行为不变
- 匹配语义从 substring 改为 exact 时需确认无动态名称

**风险**：低
**验证**：`cargo test -p luft-adapters` + `cargo clippy`

---

#### 4.2.3 run.rs unwrap 审计（P2，已完成）

**现状**：

原审计把 `#[cfg(test)]` 代码计入生产路径，报告为 107 个 unwrap。当前复核显示 106 个调用全部位于测试模块；生产代码中的 metadata 序列化 unwrap 已改为带上下文的 `?`。

**策略**：不追求零 unwrap，而是**区分安全与不安全**：

1. **安全 unwrap**（如 `Uuid::now_v7()`, `Default::default()`）→ 保留但加注释或用 `expect("...")`
2. **不安全 unwrap**（如 `serde_json::from_str().unwrap()`, `std::fs::read().unwrap()`）→ 替换为 `?` + `LuftError` 变体
3. **测试代码中的 unwrap** → 保留（不影响可读性）

**before/after 示例**：

```rust
// before — run.rs 某处（示意）
let slug = derive_slug(wf, nl);
let dir_name = compose(&slug, ts);
let path = base_dir.join(&dir_name);
std::fs::create_dir_all(&path).unwrap();  // ← panic 风险

// after
let slug = derive_slug(wf, nl);
let dir_name = compose(&slug, ts);
let path = base_dir.join(&dir_name);
std::fs::create_dir_all(&path)
    .map_err(|e| LuftError::RunDirCreate(path.clone(), e))?;
```

**已修改文件**：`crates/luft/src/run.rs`

**行为不变量**：
- 成功路径行为不变
- 失败路径从 panic 改为返回 `Err`

**风险**：中 — 需确保调用链上游正确处理新的 Err 变体
**验证**：`cargo test -p luft` + `cargo clippy`

---

### 4.3 状态与持久化

#### 4.3.1 方案：日志 run_id 全链路覆盖（P5）

**现状**（来自 `docs/dev/logging-review.md`）：

| 盲区 | 原因 |
|------|------|
| `Runtime::execute` 入口 | `Runtime` 结构体无 `run_id` 字段 |
| `Planner::plan_workflow` | 函数签名无 `run_id` 参数 |
| `Journal::flush/close` | 调用链未传递 `run_id` |

总覆盖率：**48%**（14/29 处带 run_id）

**改进方案**：

1. **Runtime 结构体增加 `run_id: RunId` 字段**

```
// before
pub struct Runtime { scheduler, run_ctx, args, limits, journal, handle }
// execute() 通过 run_ctx.run_id 间接获取

// after — run_id 作为一等字段
pub struct Runtime { scheduler, run_ctx, run_id: RunId, args, limits, journal, handle }
// execute() 入口直接 tracing::info!(%run_id, ...)
```

2. **Planner 函数签名增加 `run_id`**

```
// before
pub async fn plan_workflow(nl: &str, backend, cfg) -> Result<PlanResult>
// after
pub async fn plan_workflow(nl: &str, backend, cfg, run_id: RunId) -> Result<PlanResult>
```

3. **Journal flush/close 携带 run_id**

在 `JournalStore::flush` / `close` 方法中接收 `run_id` 参数或在已有上下文中获取。

**拟修改文件**：
- `crates/luft-runtime/src/sandbox.rs` — Runtime 结构体 + execute()
- `crates/luft-planner/src/lib.rs` — plan_workflow 签名
- `crates/luft-core/src/journal.rs` — flush/close 方法
- 调用链中所有传递点

**行为不变量**：
- 只新增日志输出，不改变控制流
- run_id 值来源于已有的 `run_ctx.run_id`，不引入新的 ID

**风险**：低 — 纯添加日志参数
**验证**：
- 运行 `luft run "hello world"` 并检查 `luft.log` 中每行都包含 `run_id=`
- `grep -c run_id luf.log` 应匹配总日志行数

---

### 4.4 CLI / MCP / Service

#### 4.4.1 旧审计项：connect_timeout（P6，已解决）

**现状**：

```
// acp_adapter.rs:341
connect_timeout: config.connect_timeout,  // 赋值到 state

// initialize handshake (acp_adapter.rs:709-721) 已消费该配置
// 旧审计结论已过期
```

**结论**：无需修改运行时代码。后续应补充慢 initialize 测试，覆盖 timeout 错误信息。
**验证**：配一个不可达地址，确认超时行为

---

#### 4.4.2 CLI 命令文档对齐（P9，旧审计待复核）

**现状**：

`luft-cli/src/main.rs:1-10` 的模块文档列出了 CLI 命令概要，但与 `commands/mod.rs` 的 18 个子模块未完全对齐。

**改进方案**：
- 将 `main.rs` 模块注释更新为完整的子命令表
- 每个子命令 handler 顶部加 `/// CLI: luft <name>` 注释行
- 确保 `docs/architecture/commands.md` 与源码一致

**拟修改文件**：
- `crates/luft-cli/src/main.rs` — 模块文档
- `crates/luft-cli/src/commands/mod.rs` — 子模块文档
- `docs/architecture/commands.md`

**行为不变量**：不适用（纯文档）
**风险**：无
**验证**：人工审阅

---

### 4.5 测试 / 文档 / 命名

#### 4.5.1 测试覆盖审计（P8 已完成，P9 结论过期）

**现状**：

| 文件 | 行数 | 测试数 |
|------|------|--------|
| `luft-cli/src/main.rs` | 当前源码 | 326（325 passed, 1 ignored） |
| `luft/src/query.rs` | 当前源码 | 4 个 query 边界测试 |
| `luft-core/src/testing.rs` | 203 | 0 |

**改进方案**：

- **`query.rs`**：已补充空库、缺失 run、事件/发现/报告错误路径测试
- **`main.rs`**：现有模块测试已覆盖 CLI 路由；“0 测试”结论已过期
- **`testing.rs`**：这是测试工具本身 — 至少写构造和序列化测试

**拟修改文件**：
- `crates/luft/src/query.rs` — 新增 `#[cfg(test)] mod tests`
- `crates/luft-cli/tests/cli_smoke.rs` — 新建
- `crates/luft-core/src/testing.rs` — 新增 `#[cfg(test)] mod tests`

**行为不变量**：不适用（新增测试）
**风险**：无
**验证**：`cargo test --workspace` 新测试通过

---

#### 4.5.2 方案：luft-core lib.rs 文档修正（P11，已完成）

**现状**：

```rust
// luft-core/src/lib.rs:5
//! `luft-core` is the dependency-aware foundation of the Luft ecosystem.
```

实际依赖：`tokio`, `dashmap`, `chrono`, `tokio-util`, `serde`, `thiserror` 等。

**改进后**：

```rust
//! `luft-core` is the foundation of the Luft ecosystem.
//! It has no Luft-internal dependencies — only external crates
//! (tokio, dashmap, serde, chrono, thiserror).
```

**已修改文件**：`crates/luft-core/src/lib.rs:5-8`
**风险**：无

---

#### 4.5.3 方案：converge.rs 移除 `#![allow(dead_code)]`（P7）

**现状**：

```rust
// converge.rs:1
#![allow(dead_code)]
```

整个 1448 行模块被标记为允许死代码 — 可能是开发阶段遗留。

**改进方案**：
1. 移除 `#![allow(dead_code)]`
2. 运行 `cargo build` 查看实际死代码警告
3. 对真正未使用的函数：删除或标注 `#[allow(dead_code)]` + 注释说明保留原因
4. 对实际被使用的函数：确认无问题

**拟修改文件**：`crates/luft-runtime/src/converge.rs:1` + 触发的死代码项
**行为不变量**：不改变运行时行为
**风险**：低
**验证**：`cargo build -p luft-runtime` 无警告

---

## 5. 路线图

### 阶段 1：Quick Wins（1-2 天）

低风险、无行为变更、立即可做：

| # | 任务 | 文件 | 预计 |
|---|------|------|------|
| Q1 | 修正 lib.rs "zero-dependency" 文档 | `luft-core/src/lib.rs:5-8` | **已完成** |
| Q2 | 修复 `result_collector.rs:24` 缩进 | `result_collector.rs:24` | **已完成** |
| Q3 | 移除 `converge.rs:1` `#![allow(dead_code)]` 并清理 | `converge.rs` | **已完成** |
| Q4 | structured_output 魔法字符串提取为常量 | adapters 内部常量 + 使用点 | **已完成** |
| Q5 | CLI main.rs 文档对齐 | `main.rs`, `commands/mod.rs` | **原审计结论已复核并标记过期** |

### 阶段 2：局部重构（3-5 天）

中等风险、需测试验证：

| # | 任务 | 涉及文件 | 预计 |
|---|------|----------|------|
| L1 | Accumulator 封装 | `update_mapper.rs`, `acp_adapter.rs`, `result_collector.rs` | **已完成** |
| L2 | StopReason 类型安全恢复 | `acp_adapter.rs`, `result_collector.rs` | **已完成** |
| L3 | run.rs unwrap 分批清理 | `luft/src/run.rs` | **已完成（生产路径无 unwrap）** |
| L4 | query.rs 补测试 | `luft/src/query.rs` | **已完成** |
| L5 | 日志 run_id 全链路覆盖 | `sandbox.rs`, `planner/lib.rs`, `journal.rs` | **已完成（flush 无操作边界已记录）** |

### 阶段 3：架构级改动（需讨论后执行）

高影响、需用户决策：

| # | 任务 | 说明 |
|---|------|------|
| A1 | permission.rs 结构化输出绕过策略 | **待决策**：无条件放行是否可接受？ |
| A2 | event.rs 枚举拆分 | 1140 行单文件是否需要按事件类型拆分？ |

---

## 6. 不建议现在做的改动

| 改动 | 原因 |
|------|------|
| 将 `luft-core` 拆分为 `luft-contract` + `luft-scheduler` | 影响面太大（所有 crate 都依赖 luft-core），收益不足以抵消风险 |
| 替换 mlua 为其他 Lua 引擎 | 运行时核心稳定，无动机 |
| 将 `AgentEvent` 枚举改为 trait object | 序列化兼容性风险，`#[serde(tag="type")]` 工作良好 |
| 引入 `anyhow` 替换各 crate 自定义 Error 类型 | 各 crate 已用 `thiserror`，层次清晰 |
| 合并 `luft-daemon` 和 `luft-mcp` | 职责不同（WS+HTTP vs stdio），当前分离合理 |
| 重写 converge 算法 | 功能正常，仅可读性改进（移除 allow(dead_code)） |

---

## 7. 验收标准与策略

### 7.1 验收标准

| 指标 | 当前值 | 目标值 |
|------|--------|--------|
| 日志 run_id 覆盖率 | 48% | ≥ 90% |
| `run.rs` 生产 unwrap 数 | 0 | 0 |
| StopReason 子串匹配 | 0（已改用 match） | 0 |
| structured-output 工具名硬编码 | 0（已统一 crate 常量） | 0 |
| >200 行零测试文件 | 6 个 | ≤ 2 个 |
| `#![allow(dead_code)]` | converge.rs 全局 | 移除或精确标注 |
| Accumulator 公共 Mutex 字段 | 3 个 | 0（已封装） |

### 7.2 测试策略

- **每个 PR 必须通过** `cargo test --workspace` + `cargo clippy --workspace`
- **重构类改动**：先补充现有行为的快照测试，再重构，确保快照不变
- **日志类改动**：用 `tracing_subscriber::layer()` 捕获日志行，断言 run_id 存在
- **Accumulator 封装**：并发测试（多线程 append_message + snapshot）

### 7.3 回滚策略

- 所有改动按独立 PR 提交，不混合多个主题
- 每个阶段完成后打 git tag（`readability-phase-1`, `readability-phase-2`）
- 如某 PR 引入回归：`git revert <PR>` 即可回滚，不影响其他改动
- Accumulator/StopReason 类改动如回滚：恢复 public 字段 / String 传递即可

### 7.4 待用户决策的问题

| # | 问题 | 选项 | 建议 |
|---|------|------|------|
| D1 | `connect_timeout` | A) 保持当前 handshake timeout / B) 补充慢 initialize 测试 | **建议 B** |
| D2 | `permission.rs:39` 无条件放行 | A) 保留（当前行为）/ B) 改为可配置白名单 | **建议 A**（v0.1 限制内可接受） |
| D3 | `event.rs` 是否拆分 | A) 保留单文件 / B) 按 Run/Phase/Agent 拆分 | **建议 A**（当前 1140 行可管理） |
| D4 | 空 crate `workflow-*` | A) 从 workspace 移除 / B) 保留占位 | **建议 A**（无代码价值） |
| D5 | `run.rs` 是否拆分 | A) 保留单文件 / B) 拆为 resolve.rs + prepare.rs + execute.rs | **暂不需要**（生产 unwrap 已清零，是否拆分仍可独立评估） |

---

## 8. 附录：证据索引

### A.1 文件行数统计（Top 20）

数据来源：`Get-Content | Measure-Object -Line`，2025-08-19。

| 文件 | 行数 |
|------|------|
| `crates/luft-adapters/src/acp_adapter.rs` | 1595 |
| `crates/luft-cli/src/commands/artifact_writer.rs` | 1589 |
| `crates/luft-runtime/src/converge.rs` | 1448 |
| `crates/luft/src/run.rs` | 1356 |
| `crates/luft-storage/src/writer.rs` | 1272 |
| `crates/luft-core/src/contract/event.rs` | 1140 |
| `crates/luft-cli/src/commands/run.rs` | 1097 |
| `crates/luft-service/src/service.rs` | 1063 |
| `crates/luft-core/src/scheduler/mod.rs` | 1049 |
| `crates/luft-cli/src/commands/phase_renderer.rs` | 1042 |
| `crates/luft/src/builder.rs` | 1018 |
| `crates/luft-storage/src/checkpoint.rs` | 884 |
| `crates/luft-cli/src/install/mcp_setup.rs` | 846 |
| `crates/luft/src/phases.rs` | 808 |
| `crates/luft-runtime/src/sandbox.rs` | 774 |
| `crates/luft-runtime/src/sdk/agent/pmap.rs` | 749 |
| `crates/luft-runtime/src/sdk/agent/parallel.rs` | 718 |
| `crates/luft-planner/src/lib.rs` | 714 |
| `crates/luft-cli/src/commands/event_log.rs` | 693 |
| `crates/luft-adapters/src/update_mapper.rs` | 662 |

### A.2 unwrap / clone 密集文件（Top 15）

| 文件 | unwrap | clone |
|------|--------|-------|
| `crates/luft/src/run.rs` | **106（仅测试）** | 16 |
| `crates/luft-cli/src/commands/artifact_writer.rs` | **103** | 22 |
| `crates/luft-runtime/src/converge.rs` | **101** | 25 |
| `crates/luft-core/src/contract/event.rs` | 83 | 2 |
| `crates/luft-runtime/src/sdk/control.rs` | 75 | 2 |
| `crates/luft-runtime/src/sdk/sandbox.rs` | 75 | 2 |
| `crates/luft-runtime/src/sdk/agent/parallel.rs` | 74 | 18 |
| `crates/luft-storage/src/writer.rs` | 65 | 3 |
| `crates/luft-cli/src/commands/run.rs` | 52 | 22 |
| `crates/luft-runtime/src/sandbox.rs` | 51 | 11 |
| `crates/luft-cli/src/install/mcp_setup.rs` | 51 | 4 |
| `crates/luft-mcp/src/resources.rs` | 48 | 0 |
| `crates/luft-adapters/src/update_mapper.rs` | 41 | 7 |
| `crates/luft-runtime/src/sdk/agent/pmap.rs` | 41 | 15 |
| `crates/luft-runtime/src/sdk/report.rs` | 39 | 4 |

### A.3 零测试大文件（>200 行，0 个 `#[test]`）

| 文件 | 行数 |
|------|------|
| `crates/luft-cli/src/main.rs` | 501 |
| `crates/luft-mcp/tests/mcp_e2e.rs` | 410 |
| `crates/luft/src/query.rs` | 321 |
| `crates/luft-cli/tests/runtime_e2e.rs` | 299 |
| `crates/luft/tests/lifecycle.rs` | 290 |
| `crates/luft-core/src/testing.rs` | 203 |

### A.4 关键问题行号索引

| 问题 | 文件:行号 | 证据 |
|------|-----------|------|
| StopReason Debug-format | `crates/luft-adapters/src/acp_adapter.rs:364` | `format!("{:?}", stop_reason)` |
| StopReason 子串匹配 | 历史实现（已移除） | `StopReasonKind` 显式 match |
| Accumulator 公共 Mutex | `crates/luft-adapters/src/update_mapper.rs:16-26` | `pub message: Mutex<String>` |
| structured_output 策略绕过 | `crates/luft-adapters/src/permission.rs:38-41` | `tool == "workflow_validate_schema"` → Approve |
| connect_timeout 已消费 | `crates/luft-adapters/src/acp_adapter.rs:709-721` | initialize 使用 `tokio::time::timeout` |
| converge 全局 allow(dead_code) | `crates/luft-runtime/src/converge.rs:1` | `#![allow(dead_code)]` |
| lib.rs 依赖描述 | `crates/luft-core/src/lib.rs:5-8` | `"dependency-aware foundation"` |
| result_collector 缩进错误 | `crates/luft-adapters/src/result_collector.rs:24` | `let output = if` 未缩进 |

### A.5 现有审查文档索引

| 文档 | 路径 | 内容 |
|------|------|------|
| Adapters 异味审计 | `docs/dev/adapters-smell-audit-report.json` | 评分 6/10，5 类系统性问题 |
| 日志系统审计 | `docs/dev/logging-review.md` | run_id 覆盖率 48%，3 类盲区 |
| 架构总览 | `docs/architecture.md` | 8 模块索引 + 依赖图 |
| Core 架构 | `docs/architecture/core.md` | 骨架文档（标注"待完善"） |
| Runtime 架构 | `docs/architecture/runtime.md` | 执行链路 + SDK 原语 |
| Storage 架构 | `docs/architecture/storage.md` | SQLite 7 表 + 查询 API |
| Library Split 设计 | `docs/design/library-split.md` | crate 拆分历史依据 |
| SQLite 迁移设计 | `docs/design/sqlite-checkpoint-events.md` | 持久化迁移方案 |

### A.6 Workspace Crate 清单（11 个）

| Crate | 职责 | 源文件数 |
|-------|------|----------|
| `luft` | Facade — LuftBuilder/Luft/RunHandle + run 生命周期 | 7 |
| `luft-core` | 冻结合约 + 调度器 + Journal + State | 18 |
| `luft-runtime` | mlua VM + SDK 原语 + pipeline + converge | ~15 |
| `luft-adapters` | ACP 后端实现（opencode/codex） | 5 |
| `luft-planner` | NL → Lua 脚本生成 + 校验重试 | 1 |
| `luft-service` | WorkflowService trait + 业务编排 | ~5 |
| `luft-storage` | SQLite 持久化（7 表 + 读写 API） | 5 |
| `luft-cli` | CLI 二进制 + 18 子命令 | ~25 |
| `luft-daemon` | WebSocket + HTTP daemon | 7 |
| `luft-mcp` | MCP stdio server（rmcp） | 5 |
| `luft-skills` | 内置 Skill 定义 | **(待确认)** |
