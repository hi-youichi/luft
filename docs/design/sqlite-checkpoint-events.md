# SQLite 统一存储方案：checkpoint + events

## 目标

**废弃 `checkpoint.json` + `events.jsonl`，全部进 SQLite。** 文件系统仅保留 `luft.db` 一个文件。不兼容旧格式。

## 方案

### 1. 新增 `checkpoints` 表

```sql
-- 20250821000001_checkpoints.sql
CREATE TABLE checkpoints (
    run_id              BLOB    NOT NULL PRIMARY KEY REFERENCES runs(run_id) ON DELETE CASCADE,
    status              TEXT    NOT NULL DEFAULT 'running',   -- running|completed|failed|cancelled
    current_phase       INTEGER NOT NULL DEFAULT 0,
    total_tokens        INTEGER NOT NULL DEFAULT 0,
    created_at          INTEGER NOT NULL,  -- unix epoch (与现有 u64 对齐)
    updated_at          INTEGER NOT NULL,
    workflow_meta       TEXT,              -- JSON blob
    started_agent_ids   TEXT NOT NULL DEFAULT '[]'  -- JSON array of UUID strings
);

CREATE INDEX idx_checkpoints_status ON checkpoints(status);
```

### 2. 复用已有表承载 checkpoint 子结构

现有 `agents` 表已经有 `status`, `output`, `tokens`, `phase_id` — 只需 **追加 4 列** 即可完全覆盖 `AgentResultCache`：

```sql
-- 20250821000002_agent_cache_fields.sql
ALTER TABLE agents ADD COLUMN cache_key_hash  TEXT;
ALTER TABLE agents ADD COLUMN description     TEXT;
ALTER TABLE agents ADD COLUMN role            TEXT;
ALTER TABLE agents ADD COLUMN findings_json   TEXT NOT NULL DEFAULT '[]';  -- Vec<Finding> serialized
ALTER TABLE agents ADD COLUMN completed_at    INTEGER NOT NULL DEFAULT 0;
```

现有 `agents` 表已有的 `agent_sessions` 字段不足以覆盖 `AgentSessionCheckpoint`。新增一个专用表：

```sql
-- 20250821000003_agent_sessions.sql
CREATE TABLE agent_sessions (
    run_id               BLOB    NOT NULL REFERENCES runs(run_id) ON DELETE CASCADE,
    agent_id             BLOB    NOT NULL,
    backend_id           TEXT,
    protocol_session_id  TEXT,
    session_id           TEXT    NOT NULL,
    status               TEXT    NOT NULL,
    updated_at           INTEGER NOT NULL,
    resumable            INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (run_id, agent_id)
);
```

### 3. `PhaseSummary` 复用 `phases` 表

现有 `phases` 表已有 `phase_id`, `label`, `planned`, `ok`, `failed`, `description`, `role` — 完全覆盖 `PhaseSummary`，**无需改动**。

### 4. `RunStore` 重构

```rust
/// 新的 RunStore — SQLite-backed，替代文件系统版本。
pub struct RunStore {
    pool: DbPool,
    run_id: RunId,
    /// 内存缓存：agent_id → AgentResultCache（热路径读，避免每次 SELECT）
    cache: RwLock<HashMap<AgentId, AgentResultCache>>,
    /// 内存缓存：cache_key_hash → AgentResultCache（resume 快速查重）
    cache_index: RwLock<HashMap<String, AgentResultCache>>,
    /// 内存缓存：checkpoint（热路径读）
    checkpoint_cache: RwLock<Option<RunCheckpoint>>,
}
```

**关键方法映射：**

| 方法 | 旧实现 | 新实现 |
|------|--------|--------|
| `init_run` | 写 checkpoint.json + 打开 events.jsonl | INSERT runs + INSERT checkpoints |
| `append_event` | JSONL writeln + 全量重写 checkpoint.json | INSERT events + UPSERT agents/phases/checkpoints (事务) |
| `open_run` | 读 checkpoint.json + 打开 events.jsonl | SELECT checkpoints + SELECT agents → 重建内存缓存 |
| `upsert_agent_result` | 全量重写 checkpoint.json | UPSERT agents (单行) |
| `upsert_agent_session` | 全量重写 checkpoint.json | UPSERT agent_sessions (单行) |
| `cancel` | 读-改-写 checkpoint.json | UPDATE checkpoints SET status='cancelled' (原子) |
| `reset_status_to_running` | 读-改-写 checkpoint.json | UPDATE checkpoints SET status='running' (原子) |
| `get_checkpoint` | 内存 RwLock 读 | 内存缓存（SELECT 回填） |
| `get_event_log` | 逐行解析 events.jsonl | SELECT payload FROM events WHERE run_id=? ORDER BY seq |
| `save_checkpoint` | 全量写 checkpoint.json | UPSERT checkpoints (批量字段) |

**核心改进：** `cancel()` 和 `update_from_event()` 不再需要"读磁盘判断是否已 Cancelled"的竞态防护 — SQLite 的 `UPDATE ... WHERE status != 'cancelled'` 在单条 SQL 中原子完成。

### 5. `append_event` 事务化

```rust
pub async fn append_event(&self, event: &AgentEvent) -> Result<()> {
    let mut tx = self.pool.begin().await?;

    // 1. 写事件审计日志（替代 events.jsonl）
    let payload = serde_json::to_string(event)?;
    sqlx::query("INSERT INTO events (run_id, type, payload) VALUES (?, ?, ?)")
        .bind(event.run_id())
        .bind(event.type_tag())
        .bind(&payload)
        .execute(&mut *tx).await?;

    // 2. 更新结构化表（现有 EventWriter 逻辑，搬入事务）
    self.apply_event_to_tables(&mut tx, event).await?;

    // 3. 更新 checkpoint 表（替代全量重写 checkpoint.json）
    self.apply_event_to_checkpoint(&mut tx, event).await?;

    tx.commit().await?;

    // 4. 更新内存缓存
    self.update_memory_cache(event);

    Ok(())
}
```

### 6. Resume 流程

```rust
pub async fn open_run(&self, run_id: RunId) -> Result<Option<RunCheckpoint>> {
    // 单次查询重建完整 checkpoint
    let row = sqlx::query_as::<_, CheckpointRow>(
        "SELECT * FROM checkpoints WHERE run_id = ?"
    ).bind(run_id).fetch_optional(&self.pool).await?;

    let Some(cp_row) = row else { return Ok(None); };

    // 拉取 agent_results
    let agents = sqlx::query_as::<_, AgentResultCache>(
        "SELECT agent_id, phase_id, status, output, findings_json, tokens, completed_at,
                cache_key_hash, description, role
         FROM agents WHERE run_id = ?"
    ).bind(run_id).fetch_all(&self.pool).await?;

    // 拉取 agent_sessions
    let sessions = sqlx::query_as::<_, AgentSessionRow>(
        "SELECT * FROM agent_sessions WHERE run_id = ?"
    ).bind(run_id).fetch_all(&self.pool).await?;

    // 拉取 phases
    let phases = sqlx::query_as::<_, PhaseSummary>(
        "SELECT phase_id, label, planned, ok, failed, description, role
         FROM phases WHERE run_id = ?"
    ).bind(run_id).fetch_all(&self.pool).await?;

    // 组装 RunCheckpoint
    let checkpoint = RunCheckpoint {
        run_id,
        task: cp_row.task,          // 从 runs 表获取
        status: cp_row.status.into(),
        current_phase: cp_row.current_phase,
        completed_phases: phases,
        agent_results: agents.into_iter().map(|a| (a.agent_id, a)).collect(),
        agent_sessions: sessions.into_iter().map(|s| (s.agent_id, s)).collect(),
        findings: vec![],           // 从 findings 表拉取
        total_tokens: cp_row.total_tokens,
        created_at: cp_row.created_at,
        updated_at: cp_row.updated_at,
        workflow_meta: cp_row.workflow_meta,
        started_agent_ids: serde_json::from_str(&cp_row.started_agent_ids)?,
    };

    // 重建内存索引
    self.rebuild_cache_index(&checkpoint);

    Ok(Some(checkpoint))
}
```

### 7. 删除的文件 / 代码

| 文件/代码 | 处理 |
|-----------|------|
| `state.rs` 中的文件 I/O 逻辑（`fs::write`, `OpenOptions`, `BufReader`） | 删除 |
| `write_checkpoint_to_disk()` | 删除 |
| `events.jsonl` 追加逻辑 | 删除 |
| `run.rs:389-411` 中的 dual-write forwarder | 简化为单写 SQLite |
| `RunStore::events_file: RwLock<Option<File>>` 字段 | 删除 |
| 跨进程 cancel 的磁盘读竞态防护（`state.rs:358-416`） | 删除（SQL 原子操作替代） |

### 8. RunStore → async 迁移

现有 `RunStore` 方法是 **同步的**（返回 `io::Result`），因为文件 I/O 可以在同步上下文完成。迁移到 SQLite (sqlx) 后，所有方法变为 **async**。

**影响面：**
- `JournalStore` 的所有方法变为 async
- `JournalCallback` trait 已经是 async（`#[async_trait]`），无需改动
- `run.rs` 中的 forwarder 已经是 async tokio task，适配即可
- `query.rs` 中的 `get_checkpoint()` 改为 `async fn`

**同步调用点：**
- `luft-cli` 中的 `status` / `logs` / `phases` 命令需要在 `#[tokio::main]` 或 `block_on` 中调用
- `gc_runs()` 改为 async

### 9. 性能对比

| 指标 | 旧方案 (JSONL + JSON) | 新方案 (SQLite) |
|------|----------------------|-----------------|
| 每事件写入 I/O | 1× append + 1× 全量重写 (~10KB) | 1× INSERT (事务批量) |
| checkpoint 读 | 全量解析 JSON | SELECT 指定列 |
| cancel() 跨进程 | 读-判断-写 (3 步) | UPDATE ... WHERE (1 步, 原子) |
| Resume 启动 | 解析 JSON + 逐行回放 JSONL | 3 条 SELECT |
| 事件查询 | 逐行扫描 JSONL | SELECT + WHERE + INDEX |
| 并发写 | RwLock + 文件锁 | SQLite WAL (行级并发) |

## 实施计划

### Phase 1: Schema
1. 新增 3 个 migration 文件（checkpoints 表 + agents 追加列 + agent_sessions 表）

### Phase 2: RunStore 重构
1. 在 `luft-storage` 中新增 `run_store.rs`，实现 `SqliteRunStore`
2. 复用 `EventWriter` 的 `apply_event_to_tables` 逻辑
3. 保持与旧 `RunStore` 相同的公开 API（方法名/语义不变），但改为 async
4. 完整单元测试覆盖（移植现有 `state.rs` 的 20+ 测试）

### Phase 3: 接入运行时
1. `JournalStore` 内部从 `RunStore` 切换到 `SqliteRunStore`
2. `run.rs` 删除 JSONL forwarder，保留 SQLite 单写
3. `query.rs` 改为从 SQLite 读
4. `gc_runs()` 改为 `DELETE FROM runs WHERE ...` + `VACUUM`

### Phase 4: 清理
1. 删除 `state.rs` 中的文件 I/O 代码
2. 删除 `events.jsonl` / `checkpoint.json` 写入路径
3. 更新 CLI 帮助文本（`--resume` 不再需要指定 run_dir）

## 风险与缓解

| 风险 | 缓解 |
|------|------|
| async 迁移波及面大 | JournalStore 保持同步外观（内部 block_on），仅在 run.rs 中 async |
| SQLite 单文件损坏 = 全部丢失 | WAL + `PRAGMA synchronous=NORMAL` 已足够；可加 `.backup` API |
| sqlx 编译时间增加 | 已在依赖中（luft-storage），无新增 |
