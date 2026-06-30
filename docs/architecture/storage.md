# storage 模块架构

> **SQLite 结构化持久化。** 把 `AgentEvent` 流转化为关系型表结构，提供 UI-ready 的查询 API。与 `core::state` 的 JSONL + checkpoint.json 互补——后者是 resume 的 source of truth，前者是查询和展示的索引层。

源码：[`src/storage/`](../../src/storage/) — `db.rs`（连接池 + 迁移）、`writer.rs`（事件→SQL）、`reader.rs`（查询 API）、`error.rs`（错误类型）

---

## 1. 职责与边界

`storage` 是一个**可选的结构化持久化层**——如果 DB 打开失败（只读文件系统等），系统继续运行，journal + JSONL 保持为唯一持久化机制。

```
   service::run::prepare()
       │
       ▼
   open_db() ──► DbPool (WAL, FK, busy_timeout=5s)
       │
       ├── EventWriter::write_event(AgentEvent)  ← 由事件转发任务驱动
       │       └── INSERT INTO runs / phases / agents / turns / spans / findings / events
       │
       └── reader::get_run_overview(pool, run_id)  ← 由 UI / 查询 API 调用
               └── SELECT ... FROM runs JOIN agents JOIN turns ...
```

**边界**：`storage` 只依赖 `core::contract`（类型定义）和 `sqlx`。不知道 `service` 或 `commands` 的存在。

---

## 2. 数据库配置

| 配置项 | 值 | 说明 |
|--------|------|------|
| 引擎 | SQLite (via `sqlx`) | 嵌入式，无外部服务 |
| 日志模式 | WAL | 并发读写不阻塞 |
| 同步模式 | Normal | 性能与持久性平衡 |
| busy_timeout | 5s | 写锁竞争时等待 |
| 外键 | ON | 级联删除（run 删除时清理子表） |
| 连接池 | max=8 | 并发查询 |
| DB 路径 | `.maestro/maestro.db` | 相对当前工作目录 |

迁移通过 `sqlx::migrate!("./migrations")` 在编译时嵌入，运行时自动执行。

---

## 3. Schema（7 张表 + 2 次迁移）

### 3.1 核心表

```
runs                    顶层 run 元数据
├── phases              per-run 阶段摘要（label, planned/ok/failed 计数）
├── agents              per-(run,agent) 执行详情（model, tokens, status, elapsed）
├── turns               UI 会话流核心表（per ProgressDelta，含 tool_call/diff）
├── spans               编排原语（parallel/converge/workflow/pipeline）
├── findings            结构化发现（kind, severity, file_path, evidence）
└── events              完整 AgentEvent 审计日志（replay/forensics 用）
```

### 3.2 迁移历史

| 迁移 | 内容 |
|------|------|
| `20250819000001_initial.sql` | 初始 7 表 + 索引 |
| `20250820000001_phase_meta.sql` | phases 表增加 `description`、`role` 列 |

### 3.3 turns 表设计

`turns` 是最复杂的表，每行对应一个 `ProgressDelta` 事件，覆盖：

- 文本消息（role, text）
- 工具调用（tool_call_id, name, input, output, tool_status）
- 文件编辑（file_path, file_op, diff）
- token 计数（input/output/cache_read/cache_write）

---

## 4. writer — 事件写入路径

`EventWriter` 将 `AgentEvent` 枚举的每个变体映射为 SQL 写入：

| AgentEvent | 写入目标 |
|------------|---------|
| `RunStarted` | INSERT INTO runs |
| `PhaseStarted` / `PhaseDone` | INSERT/UPDATE phases |
| `AgentStarted` | INSERT INTO agents |
| `ProgressDelta` | INSERT INTO turns |
| `AgentDone` | UPDATE agents (status, tokens, output) |
| `SpanStarted` / `SpanDone` | INSERT/UPDATE spans |
| `RunDone` | UPDATE runs (status, finished_ts, report) |
| `FindingReported` | INSERT INTO findings |
| 其他 | INSERT INTO events（审计日志） |

`EventWriter` 是 `Clone` 的（`DbPool` 内部 `Arc`），通过 `Arc<EventWriter>` 共享。

---

## 5. reader — UI 查询 API

### 5.1 查询函数

| 函数 | 返回 | 说明 |
|------|------|------|
| `list_runs(pool, limit, offset)` | `Vec<RunSummary>` | 分页列表，按 started_ts DESC |
| `get_run_overview(pool, run_id)` | `RunOverview` | run + agents + turn 统计 |
| `get_run_agents(pool, run_id)` | `Vec<AgentOverview>` | per-agent 详情 |
| `get_agent_overview(pool, run_id, agent_id)` | `AgentOverview` | 单个 agent |
| `get_agent_turns(pool, run_id, agent_id)` | `Vec<TurnRow>` | agent 会话流 |
| `get_run_spans(pool, run_id)` | `Vec<SpanRow>` | 编排原语列表 |
| `get_run_tree(pool, run_id)` | `Vec<SpanRow>` | 带 parent 关系的 span 树 |
| `search_turns(pool, run_id, query)` | `Vec<TurnRow>` | 全文搜索 |

### 5.2 DTO 类型

- `RunSummary` — 列表视图（run_id, task, status, tokens, elapsed）
- `RunOverview` — 聚合视图（run + agents + turn_counts + 统计）
- `AgentOverview` — agent 详情（model, status, tokens, elapsed）
- `TurnRow` — 会话流单行（seq, kind, text/tool_call/diff）
- `SpanRow` — 编排原语（kind, items, rounds, converged）

---

## 6. 与 core::state 的关系

| | `core::state` (RunStore) | `storage` (SQLite) |
|---|---|---|
| 格式 | JSONL + checkpoint.json | SQLite 关系表 |
| 角色 | **resume 的 source of truth** | 查询/展示索引 |
| 可选性 | 必须（journal 依赖） | 可选（失败时 warn 并降级） |
| 写入时机 | `prepare()` 内事件转发任务 | 同一个转发任务，双写 |
| 读取方 | `service::query` | `storage::reader`（UI 用） |

**设计决策**：双写而非替代。JSONL 是面向恢复的追加日志，SQLite 是面向查询的关系存储。两者接收完全相同的事件流，各自维护自己的投影。

---

## 7. 相关文档

- 总览：[../architecture.md](../architecture.md)
- 调用方：[service.md](./service.md)（`prepare()` 创建 writer，`query` 读 JSONL）
- 事件源：[core.md](./core.md)（AgentEvent 定义）
