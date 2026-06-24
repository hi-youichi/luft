# SQLite 持久化方案

> **状态**: 方案设计
> **目标**: 用 SQLite（sqlx）替换现有的 JSONL + checkpoint.json 文件持久化，为 UI 提供结构化、可查询的 agent 交互数据存储
> **交叉参考**: [交互结构化方案](./structured-interaction-persistence.md)、[事件日志](./event-logging.md)、[程序日志](./program-logging.md)
> **相关代码**: [`src/core/state.rs`](../../src/core/state.rs)、[`src/core/journal.rs`](../../src/core/journal.rs)、[`src/service/run.rs`](../../src/service/run.rs)、[`src/service/query.rs`](../../src/service/query.rs)

---

## 0. 为什么用 SQLite

当前持久化基于两个文件：`events.jsonl`（事件追加日志）+ `checkpoint.json`（run 状态快照）。问题：

- **查询能力为零**：`get_event_log()`（[`state.rs:293-310`](../../src/core/state.rs#L293-L310)）全量读入内存再过滤；无分页、无索引、无聚合
- **并发受限**：`RunStore` 用 `RwLock<File>` 保护单文件句柄（[`state.rs:73-77`](../../src/core/state.rs#L73-L77)），WAL 无从谈起
- **信息太薄**：`ProgressDelta` 字段不足（详见 [交互结构化方案](./structured-interaction-persistence.md) §0），且 JSONL 无法表达关系（tool_call ↔ tool_result 关联）
- **跨 run 查询不可能**：`list_runs()`（[`state.rs:378-396`](../../src/core/state.rs#L378-L396)）扫目录；run 列表页需要逐个打开 checkpoint.json

SQLite + WAL 解决以上全部：原生 SQL 查询/分页/聚合、读写并发、跨 run 查询、外键关系。

---

## 1. 拓扑：全局单 DB

```
.maestro/
  maestro.db          ← 全局唯一数据库
  maestro.db-wal      ← WAL 文件（自动管理）
  maestro.db-shm      ← 共享内存（自动管理）
  runs/{run_id}/      ← 保留：agent 工作目录、临时文件等非 DB 数据
```

**决策理由**：
- UI 的 run 列表页是高频操作，全局 DB 一条 `SELECT` 搞定
- WAL 模式下读写不互斥，UI 可以实时查看正在运行的 agent（读 WAL 不阻塞写）
- SQLite 单文件到 GB 级别无压力（一个 turn 约 500B，100 万 turns ≈ 500MB）
- `runs/{run_id}/` 目录保留，用于 agent 工作区、临时文件、输出产物等

---

## 2. 依赖

```toml
# Cargo.toml [dependencies]
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "chrono", "uuid", "json"] }
```

- `runtime-tokio`：与现有 async runtime 对齐
- `sqlite`：用 bundled SQLite（`sqlx-sqlite` 默认 bundle）
- `chrono`：`DateTime<Utc>` 直接映射
- `uuid`：`Uuid` 直接映射
- `json`：`serde_json::Value` 映射为 `TEXT`（JSON 类型）

---

## 3. Schema

### 3.1 `runs` — run 元数据

```sql
CREATE TABLE runs (
    run_id        BLOB    PRIMARY KEY,   -- UUID v7 (16 bytes)
    task          TEXT    NOT NULL,
    status        TEXT    NOT NULL DEFAULT 'running',  -- running|completed|failed|cancelled|partial
    started_ts    TEXT    NOT NULL,      -- ISO 8601
    finished_ts   TEXT,
    elapsed_ms    INTEGER NOT NULL DEFAULT 0,
    input_tokens     INTEGER NOT NULL DEFAULT 0,
    output_tokens    INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    report        TEXT,                  -- JSON
    script_path   TEXT,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
```

### 3.2 `phases` — 阶段

```sql
CREATE TABLE phases (
    run_id     BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    phase_id   INTEGER NOT NULL,
    label      TEXT    NOT NULL,
    planned    INTEGER NOT NULL DEFAULT 0,
    ok         INTEGER NOT NULL DEFAULT 0,
    failed     INTEGER NOT NULL DEFAULT 0,
    started_ts TEXT,
    done_ts    TEXT,
    PRIMARY KEY (run_id, phase_id)
);
```

### 3.3 `agents` — agent 概要（AgentStarted → AgentDone 的聚合视图）

```sql
CREATE TABLE agents (
    run_id     BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    agent_id   BLOB    NOT NULL,
    phase_id   INTEGER,
    model      TEXT,
    status     TEXT    NOT NULL DEFAULT 'running',  -- running|ok|error|cancelled|timed_out
    prompt_preview TEXT,
    input_tokens     INTEGER NOT NULL DEFAULT 0,
    output_tokens    INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    output     TEXT,              -- JSON（agent 最终输出）
    started_ts TEXT    NOT NULL,
    done_ts    TEXT,
    elapsed_ms INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (run_id, agent_id)
);

CREATE INDEX idx_agents_run ON agents(run_id);
CREATE INDEX idx_agents_phase ON agents(run_id, phase_id);
```

### 3.4 `turns` — 核心表：agent 交互 turn

UI 对话流的直接数据源。每行是一个 conversation turn（消息/工具调用/工具结果/文件编辑）。

```sql
CREATE TABLE turns (
    seq          INTEGER PRIMARY KEY AUTOINCREMENT,  -- 全局递增，自然因果序
    run_id       BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    agent_id     BLOB    NOT NULL,
    phase_id     INTEGER,
    ts           TEXT    NOT NULL,           -- ISO 8601
    kind         TEXT    NOT NULL,           -- message|tool_call|tool_result|file_edit|tokens
    -- message 字段
    role         TEXT,                       -- assistant|reasoning|user
    text         TEXT,                       -- 消息正文
    -- tool_call / tool_result 字段
    tool_call_id TEXT,                       -- 关联 tool_call ↔ tool_result
    name         TEXT,                       -- 工具名称
    input        TEXT,                       -- JSON（工具入参）
    output       TEXT,                       -- JSON（工具输出）
    tool_status  TEXT,                       -- completed|failed
    -- file_edit 字段
    file_path    TEXT,
    file_op      TEXT,                       -- create|edit|delete
    diff         TEXT,
    -- tokens 字段
    input_tokens     INTEGER,
    output_tokens    INTEGER,
    cache_read_tokens INTEGER,
    cache_write_tokens INTEGER
);

CREATE INDEX idx_turns_run_agent ON turns(run_id, agent_id, seq);
CREATE INDEX idx_turns_run ON turns(run_id, seq);
CREATE INDEX idx_turns_tool_call ON turns(tool_call_id) WHERE tool_call_id IS NOT NULL;
```

### 3.5 `spans` — 编排层级（parallel / converge / workflow）

```sql
CREATE TABLE spans (
    run_id          BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    span_id         INTEGER NOT NULL,
    kind            TEXT    NOT NULL,        -- parallel|converge|workflow|pipeline
    phase_id        INTEGER,
    parent_span_id  INTEGER,                 -- 嵌套关系
    label           TEXT,
    path            TEXT,                    -- workflow 的脚本路径
    items           INTEGER,                 -- parallel/converge 的子项数
    max_rounds      INTEGER,                 -- converge 的最大轮次
    rounds          INTEGER,                 -- converge 实际轮次
    converged       INTEGER,                 -- converge 是否收敛 (0/1)
    ok              INTEGER,
    failed          INTEGER,
    result          TEXT,                    -- JSON
    error           TEXT,
    started_ts      TEXT,
    done_ts         TEXT,
    elapsed_ms      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (run_id, span_id)
);

CREATE INDEX idx_spans_run ON spans(run_id);
CREATE INDEX idx_spans_parent ON spans(run_id, parent_span_id);
```

### 3.6 `findings` — 结构化发现

```sql
CREATE TABLE findings (
    run_id     BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    agent_id   BLOB,
    phase_id   INTEGER,
    kind       TEXT    NOT NULL,
    severity   TEXT    NOT NULL,             -- info|low|medium|high|critical
    title      TEXT    NOT NULL,
    detail     TEXT,
    file_path  TEXT,
    line_start INTEGER,
    line_end   INTEGER,
    evidence   TEXT,                         -- JSON array
    data       TEXT,                         -- JSON
    created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (run_id, agent_id, title)
);

CREATE INDEX idx_findings_run ON findings(run_id);
CREATE INDEX idx_findings_severity ON findings(run_id, severity);
```

### 3.7 `events` — 原始事件日志（审计/回放）

保留完整的 `AgentEvent` 序列，用于审计和精确回放。与 `turns`/`agents` 等结构化表互补：结构化表为 UI 优化，`events` 表为完整性保留。

```sql
CREATE TABLE events (
    seq      INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id   BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    ts       TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    type     TEXT    NOT NULL,               -- agent_started|agent_progress|agent_done|...
    payload  TEXT    NOT NULL                -- 完整 AgentEvent JSON
);

CREATE INDEX idx_events_run ON events(run_id, seq);
```

---

## 4. 架构设计

### 4.1 模块划分

```
src/storage/
  mod.rs          — re-exports + error types
  db.rs           — DbPool 管理（连接池、初始化、migration）
  writer.rs       — EventWriter：AgentEvent → SQL 写入
  reader.rs       — 查询 API（UI-ready）
  migration.rs    — schema 迁移（sqlx migrate）
```

### 4.2 连接池

```rust
use sqlx::sqlite::{SqlitePool, SqliteConnectOptions};

pub type DbPool = SqlitePool;

pub async fn open_db(path: &Path) -> Result<DbPool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .foreign_keys(true);

    let pool = SqlitePool::connect_with(options).await?;
    sqlx::migrate!("migrations").run(&pool).await?;
    Ok(pool)
}
```

**关键参数**：
- `Wal`：读写不互斥，UI 读不阻塞 writer
- `Synchronous::Normal`：WAL 模式下的推荐级别，崩溃不丢已提交事务
- `busy_timeout(5s)`：写竞争时等待而非立即报错
- `foreign_keys(true)`：级联删除（删 run 自动清理 turns/events）

### 4.3 EventWriter — 事件写入器

替代现有的 `RunStore::append_event()` + forwarder 机制。订阅 broadcast channel，将 `AgentEvent` 转化为 SQL 写入。

```rust
pub struct EventWriter {
    pool: DbPool,
}

impl EventWriter {
    /// 处理一条 AgentEvent，写入对应的 SQL 表。
    /// 幂等：重复写入相同事件不产生副作用（UPSERT 语义）。
    pub async fn write_event(&self, event: &AgentEvent) -> Result<(), StorageError> {
        match event {
            AgentEvent::RunStarted { run_id, task, ts } => {
                sqlx::query!(
                    "INSERT INTO runs (run_id, task, status, started_ts) VALUES (?, ?, 'running', ?)",
                    run_id, task, ts
                )
                .execute(&self.pool).await?;
            }
            AgentEvent::AgentStarted { run_id, phase_id, agent_id, prompt_preview, model } => {
                sqlx::query!(
                    "INSERT OR IGNORE INTO agents (run_id, agent_id, phase_id, model, prompt_preview, status, started_ts)
                     VALUES (?, ?, ?, ?, ?, 'running', strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                    run_id, agent_id, phase_id, model, prompt_preview
                )
                .execute(&self.pool).await?;
            }
            AgentEvent::AgentProgress { run_id, agent_id, delta, .. } => {
                self.write_delta(run_id, agent_id, delta).await?;
            }
            AgentEvent::AgentDone { run_id, agent_id, status, tokens, elapsed_ms } => {
                sqlx::query!(
                    "UPDATE agents SET status = ?, input_tokens = ?, output_tokens = ?,
                     cache_read_tokens = ?, cache_write_tokens = ?, done_ts = ?, elapsed_ms = ?
                     WHERE run_id = ? AND agent_id = ?",
                    status_str(status), tokens.input, tokens.output,
                    tokens.cache_read, tokens.cache_write,
                    now_iso(), elapsed_ms, run_id, agent_id
                )
                .execute(&self.pool).await?;
            }
            AgentEvent::RunDone { run_id, status, total_tokens, report } => {
                sqlx::query!(
                    "UPDATE runs SET status = ?, finished_ts = ?, elapsed_ms = ?,
                     input_tokens = ?, output_tokens = ?, cache_read_tokens = ?, cache_write_tokens = ?,
                     report = ?
                     WHERE run_id = ?",
                    status_str(status), now_iso(), elapsed_ms,
                    total_tokens.input, total_tokens.output, total_tokens.cache_read, total_tokens.cache_write,
                    report, run_id
                )
                .execute(&self.pool).await?;
            }
            // ... PhaseStarted/PhaseDone/ParallelStarted/ParallelDone/...
            _ => {}
        }

        // 所有事件都写入 events 审计表
        self.write_audit_log(event).await?;

        Ok(())
    }

    /// 将 ProgressDelta 写入 turns 表
    async fn write_delta(&self, run_id: &RunId, agent_id: &AgentId, delta: &ProgressDelta) -> Result<(), StorageError> {
        match delta {
            ProgressDelta::Message { role, text, .. } => {
                sqlx::query!(
                    "INSERT INTO turns (run_id, agent_id, ts, kind, role, text)
                     VALUES (?, ?, ?, 'message', ?, ?)",
                    run_id, agent_id, now_iso(), role_str(role), text
                )
                .execute(&self.pool).await?;
            }
            ProgressDelta::ToolCall { tool_call_id, name, input } => {
                sqlx::query!(
                    "INSERT INTO turns (run_id, agent_id, ts, kind, tool_call_id, name, input)
                     VALUES (?, ?, ?, 'tool_call', ?, ?, ?)",
                    run_id, agent_id, now_iso(), tool_call_id, name, input
                )
                .execute(&self.pool).await?;
            }
            ProgressDelta::ToolResult { tool_call_id, status, output, elapsed_ms } => {
                sqlx::query!(
                    "INSERT INTO turns (run_id, agent_id, ts, kind, tool_call_id, tool_status, output)
                     VALUES (?, ?, ?, 'tool_result', ?, ?, ?)",
                    run_id, agent_id, now_iso(), tool_call_id, status_str(status), output
                )
                .execute(&self.pool).await?;
            }
            ProgressDelta::FileEdit { path, op, diff } => {
                sqlx::query!(
                    "INSERT INTO turns (run_id, agent_id, ts, kind, file_path, file_op, diff)
                     VALUES (?, ?, ?, 'file_edit', ?, ?, ?)",
                    run_id, agent_id, now_iso(), path_str(path), op_str(op), diff
                )
                .execute(&self.pool).await?;
            }
            ProgressDelta::Tokens { .. } => {
                // Token 更新直接写 agents 表（UPSERT 累加），不进 turns
            }
        }
        Ok(())
    }

    async fn write_audit_log(&self, event: &AgentEvent) -> Result<(), StorageError> {
        let payload = serde_json::to_string(event)?;
        let type_name = event_type_name(event);
        sqlx::query!(
            "INSERT INTO events (run_id, type, payload) VALUES (?, ?, ?)",
            event_run_id(event), type_name, payload
        )
        .execute(&self.pool).await?;
        Ok(())
    }
}
```

### 4.4 与现有系统的集成点

现有 forwarder（[`service/run.rs:254-271`](../../src/service/run.rs#L254-L271)）订阅 broadcast channel，调用 `store.append_event()`。改动：

```
 ┌─────────────┐     broadcast      ┌──────────────┐
 │  scheduler   │ ──── events ────→ │  forwarder   │
 │  + adapters  │      channel       │  (tokio spawn)│
 └─────────────┘                    └──────┬───────┘
                                           │
                              ┌────────────┴────────────┐
                              │                         │
                     当前路径（保留）           新路径（替换 append_event）
                              │                         │
                    ┌─────────▼────────┐     ┌──────────▼──────────┐
                    │  RunStore        │     │  EventWriter        │
                    │  checkpoint.json │     │  → SQLite turns/    │
                    │  events.jsonl    │     │    agents/runs/...  │
                    └──────────────────┘     └─────────────────────┘
```

**改造方案**：forwarder 中将 `store.append_event(&evt)` 替换为 `writer.write_event(&evt).await`。`RunStore` 的 checkpoint 功能（resume/cache_key）仍保留，因为 resume 语义依赖 checkpoint 快照。

### 4.5 Resume 兼容

`JournalStore::open()` 依赖 `checkpoint.json` 重建 `cache_index`。保持不变——checkpoint.json 继续作为 resume 快照，SQLite 作为查询/展示层。两者并行：

| 数据 | checkpoint.json | SQLite | 用途 |
|---|---|---|---|
| resume 快照 | ✅ 主 | ❌ 不依赖 | `--resume` 快速恢复 |
| agent 缓存索引 | ✅ 主 | ❌ | `has_completed()` / `get_cached()` |
| UI 查询/展示 | ❌ | ✅ 主 | turns 列表 / run 概览 / 对话回放 |
| 跨 run 统计 | ❌ | ✅ 主 | run 列表 / token 汇总 |

---

## 5. 查询 API（UI-Ready）

### 5.1 Run 列表页

```rust
pub async fn list_runs(pool: &DbPool, limit: i64, offset: i64) -> Result<Vec<RunSummary>> {
    let rows = sqlx::query_as!(
        RunSummary,
        r#"SELECT run_id, task, status, started_ts, finished_ts, elapsed_ms,
                  input_tokens, output_tokens
           FROM runs ORDER BY started_ts DESC LIMIT ? OFFSET ?"#,
        limit, offset
    )
    .fetch_all(pool).await?;
    Ok(rows)
}
```

### 5.2 Agent 对话流

```rust
pub async fn get_agent_turns(
    pool: &DbPool,
    run_id: &RunId,
    agent_id: &AgentId,
    limit: i64,
    offset: i64,
) -> Result<Vec<TurnRow>> {
    let rows = sqlx::query_as!(
        TurnRow,
        r#"SELECT seq, ts, kind, role, text, tool_call_id, name, input, output,
                  tool_status, file_path, file_op, diff
           FROM turns WHERE run_id = ? AND agent_id = ?
           ORDER BY seq LIMIT ? OFFSET ?"#,
        run_id, agent_id, limit, offset
    )
    .fetch_all(pool).await?;
    Ok(rows)
}
```

### 5.3 Run 概览（聚合）

```rust
pub async fn get_run_overview(pool: &DbPool, run_id: &RunId) -> Result<RunOverview> {
    let run = sqlx::query_as!(RunRow, "SELECT * FROM runs WHERE run_id = ?", run_id)
        .fetch_one(pool).await?;

    let agents = sqlx::query_as!(
        AgentRow,
        "SELECT * FROM agents WHERE run_id = ? ORDER BY started_ts",
        run_id
    )
    .fetch_all(pool).await?;

    let phase_counts = sqlx::query_as!(
        TurnCount,
        r#"SELECT kind, COUNT(*) as count FROM turns WHERE run_id = ? GROUP BY kind"#,
        run_id
    )
    .fetch_all(pool).await?;

    // 组装 RunOverview
    Ok(RunOverview { run, agents, phase_counts })
}
```

### 5.4 Run 编排树

```rust
pub async fn get_run_tree(pool: &DbPool, run_id: &RunId) -> Result<Vec<SpanRow>> {
    let spans = sqlx::query_as!(
        SpanRow,
        "SELECT * FROM spans WHERE run_id = ? ORDER BY span_id",
        run_id
    )
    .fetch_all(pool).await?;

    // 在应用层组装树（parent_span_id → children）
    Ok(spans)
}
```

### 5.5 全文搜索（turns）

SQLite 的 FTS5 可以对 turns 的 text 字段建全文索引：

```sql
CREATE VIRTUAL TABLE turns_fts USING fts5(
    text,
    content='turns',
    content_rowid='seq',
    tokenize='unicode61'
);

-- 触发器自动同步
CREATE TRIGGER turns_ai AFTER INSERT ON turns BEGIN
    INSERT INTO turns_fts(rowid, text) VALUES (new.seq, new.text);
END;
```

```rust
pub async fn search_turns(pool: &DbPool, run_id: &RunId, query: &str) -> Result<Vec<TurnRow>> {
    let rows = sqlx::query_as!(
        TurnRow,
        r#"SELECT t.seq, t.ts, t.kind, t.role, t.text, ...
           FROM turns t
           JOIN turns_fts f ON t.seq = f.rowid
           WHERE t.run_id = ? AND turns_fts MATCH ?
           ORDER BY rank"#,
        run_id, query
    )
    .fetch_all(pool).await?;
    Ok(rows)
}
```

---

## 6. Migration 策略

### 6.1 sqlx migrate

使用 `sqlx::migrate!` 宏，migrations 目录：

```
migrations/
  20250819000001_initial.sql    — 建表（§3 全部表 + 索引）
  20250819000002_fts.sql        — FTS5 虚拟表 + 触发器
```

启动时自动执行：
```rust
sqlx::migrate!("migrations").run(&pool).await?;
```

### 6.2 旧数据导入

提供 CLI 子命令 `maestro import <run_dir>`，将旧的 `events.jsonl` + `checkpoint.json` 导入 SQLite：

```rust
// 伪代码
fn import_jsonl(pool: &DbPool, events_path: &Path) -> Result<()> {
    for line in read_lines(events_path) {
        let event: AgentEvent = serde_json::from_str(&line)?;
        writer.write_event(&event).await?;
    }
    Ok(())
}
```

---

## 7. 写入策略

### 7.1 实时写

每条事件到达 forwarder 立即写入 SQLite。WAL 模式下写入极快（微秒级），不阻塞 agent 执行。

### 7.2 批量写优化（可选）

高频场景（如逐 token 的 `AgentMessageChunk`）可以微批量写入：

```rust
pub struct BufferedWriter {
    pool: DbPool,
    buffer: Mutex<Vec<AgentEvent>>,
    flush_interval: Duration,
}

// 每 100ms 或缓冲满 50 条时 flush
async fn flush_loop(&self) {
    loop {
        tokio::time::sleep(self.flush_interval).await;
        let batch = self.buffer.lock().unwrap().drain(..).collect::<Vec<_>>();
        if !batch.is_empty() {
            self.write_batch(&batch).await;
        }
    }
}

async fn write_batch(&self, events: &[AgentEvent]) -> Result<()> {
    let mut tx = self.pool.begin().await?;
    for event in events {
        // 写入 SQL（复用 write_event 逻辑，但共用同一事务）
    }
    tx.commit().await?;
    Ok(())
}
```

> **默认策略**：实时写。如果实测高频 chunk 导致写放大，再切换为微批量。

### 7.3 AcpRaw 不入库

保持现有策略：`AcpRaw` 事件不持久化（实时观测流，非历史记录）。

---

## 8. 改动文件清单

| 文件 | 改动 | 说明 |
|---|---|---|
| `Cargo.toml` | 加 `sqlx` 依赖 | features: `runtime-tokio`, `sqlite`, `chrono`, `uuid`, `json` |
| `migrations/20250819000001_initial.sql` | 新建 | §3 全部表 + 索引 |
| `migrations/20250819000002_fts.sql` | 新建 | FTS5 + 触发器 |
| `src/storage/mod.rs` | 新建 | re-exports + `StorageError` |
| `src/storage/db.rs` | 新建 | `open_db()`, 连接池管理 |
| `src/storage/writer.rs` | 新建 | `EventWriter`：AgentEvent → SQL |
| `src/storage/reader.rs` | 新建 | 查询 API（§5） |
| [`src/service/run.rs`](../../src/service/run.rs) | 改 forwarder | 替换 `store.append_event()` 为 `writer.write_event()` |
| [`src/service/query.rs`](../../src/service/query.rs) | 改查询函数 | 从 SQLite 查询替代读 JSONL |
| [`src/core/contract/event.rs`](../../src/core/contract/event.rs) | 扩展 `ProgressDelta` | 加 `role`/`tool_call_id`/`input`/`output` 等字段（见交互结构化方案） |
| [`src/adapters/update_mapper.rs`](../../src/adapters/update_mapper.rs) | 改造映射逻辑 | 提取更丰富的字段写入 `ProgressDelta` |
| [`src/core/state.rs`](../../src/core/state.rs) | 保留 | checkpoint.json 继续 用于 resume；`get_event_log()` 可以保留或弃用 |

**无需改动**：`journal.rs`（resume 逻辑不变）、`scheduler/`（不感知存储层）、`adapters/acp_adapter.rs`（只管发事件）

---

## 9. 数据生命周期

### 9.1 GC

```sql
-- 删除 N 天前的终态 run（FK ON DELETE CASCADE 自动清理关联表）
DELETE FROM runs
WHERE status IN ('completed', 'failed', 'cancelled', 'partial')
  AND finished_ts < ?;  -- cutoff timestamp
```

```rust
pub async fn gc_runs(pool: &DbPool, older_than: Duration) -> Result<u64> {
    let cutoff = Utc::now() - chrono::Duration::from_std(older_than)?;
    let result = sqlx::query!(
        "DELETE FROM runs WHERE status IN ('completed','failed','cancelled','partial') AND finished_ts < ?",
        cutoff
    )
    .execute(pool).await?;
    Ok(result.rows_affected())
}
```

### 9.2 VACUUM

定期 `VACUUM` 回收空间（可在 GC 后自动执行，或手动 CLI 命令）。

### 9.3 备份

`maestro.db` 是单文件，可以直接复制备份（WAL 模式下用 `sqlite3 .backup` 或停止写入后复制）。

---

## 10. 分阶段实施

| 阶段 | 内容 | 交付价值 |
|---|---|---|
| **P1: Schema + Writer** | 加 sqlx 依赖；建 migrations；实现 `EventWriter`；改造 forwarder | 所有 agent 交互数据进入 SQLite |
| **P2: ProgressDelta 扩展** | 扩展 `ProgressDelta` 字段 + 改造 `update_mapper` | 数据完整性（role/tool_call_id/input/output） |
| **P3: 查询 API** | 实现 `reader.rs` 的查询函数（list_runs / get_agent_turns / get_run_overview / get_run_tree） | UI 可以直接消费 |
| **P4: FTS + 旧数据迁移** | FTS5 全文索引 + `maestro import` 命令 | 搜索能力 + 历史数据不丢失 |
| **P5: checkpoint 退役（可选）** | resume 逻辑改为从 SQLite 重建 cache_index | 单一存储，消除 checkpoint.json |

---

## 11. 测试计划

- **Schema 测试**：`open_db()` 创建所有表 + 索引，FK 约束生效
- **Writer 测试**：每种 `AgentEvent` 变体 → 正确写入对应表 + `events` 审计表
- **级联删除测试**：删除 run 后 turns/agents/spans/findings 自动清理
- **Resume 兼容测试**：SQLite 写入 + checkpoint.json 并行，resume 正常工作
- **并发测试**：多个 forwarder 写入同一 DB（WAL 不死锁）
- **查询测试**：构造模拟数据，验证查询 API 返回正确结果
- **性能测试**：10 万条 turns 写入耗时 < 5s；查询 < 100ms

---

## 12. 留待后续

- **WebSocket 实时推送**：UI 订阅正在运行的 run 的 turns 流（读 WAL + notify）
- **chunk 聚合策略**：逐 token chunk 是每条都写 turns，还是按 message_id 聚合后写一条
- **大 payload 处理**：工具 input/output 可能很大，考虑 BLOB 存储 或 截断 + 外部文件引用
- **多 DB 分片**：如果数据量极大（千万级 turns），考虑按时间或 run_id 分片为多个 SQLite 文件
- **checkpoint 退役**：P5 阶段评估是否完全从 SQLite 重建 resume 状态，消除 checkpoint.json
