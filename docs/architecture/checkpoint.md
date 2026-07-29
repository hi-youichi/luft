# Checkpoint 状态持久化与恢复

> RunCheckpoint 是 Luft 工作流运行时的**全量状态快照**，持久化到磁盘 checkpoint.json。它是 --resume 的唯一数据源，同时也是终态 run 的快速查询路径。

源码: crates/luft-core/src/state.rs | crates/luft-core/src/journal.rs | crates/luft-service/src/run.rs | crates/luft-service/src/query.rs

## 1. 磁盘布局

每个 run 在 .luft/runs/<run_dir_name>/ 下维护三个文件:

- checkpoint.json - 全量状态快照 (JSON pretty-print), 每次 agent 事件到达 RunStore 时增量更新并原子写入 (temp + rename)
- events.jsonl - AgentEvent 追加写日志, 只追加不覆写; resume 时新事件继续追加到同一文件
- workflow.lua - 脚本副本, 新建 run 时写入; resume 时只读取不覆写

## 2. 核心数据结构

### 2.1 RunCheckpoint

RunCheckpoint struct 包含以下字段:

- run_id: RunId
- task: String
- status: CheckpointStatus (Running/Completed/Failed/Cancelled)
- current_phase: u32 - 当前 phase 编号, PhaseDone 事件更新
- completed_phases: Vec<PhaseSummary>
- agent_results: HashMap<AgentId, AgentResultCache> - agent_id 到结果缓存的映射
- findings: Vec<Finding>
- total_tokens: u64 - 累计 token 用量
- created_at / updated_at: u64 (UNIX timestamp)
- completed_spans: Vec<PhaseSpanSummary> - 已完成的 phase span 列表
- workflow_meta: Option<serde_json::Value> - 声明式工作流元数据
- started_agent_ids: Vec<AgentId> - 收到 AgentStarted 的 agent_id (按到达顺序)
- report: Option<serde_json::Value> - RunDone 捕获的 report 值
- event_stats: HashMap<String, u64> - 每种事件类型的计数直方图
- started_spans: Vec<StartedSpanInfo> - 已启动但未完成的 span (临时态)

### 2.2 CheckpointStatus

使用 #[serde(rename_all = "lowercase")]:
- Running -> "running" (可 resume)
- Completed -> "completed" (不可 resume)
- Failed -> "failed" (不可 resume)
- Cancelled -> "cancelled" (不可 resume)

### 2.3 AgentResultCache

每个 agent 完成后的结果快照:
- agent_id, phase_id
- status: String - 使用 AgentStatus::as_str() 生成 snake_case ("ok"/"error"/"cancelled"/"timed_out")
- output: serde_json::Value
- findings: Vec<Finding>
- tokens: u64
- completed_at: u64
- cache_key_hash: Option<String> - blake3 hex, 用于 resume 跳过
- description, role, name: Option<String>
- elapsed_ms: u64

重要: TimedOut 持久化为 "timed_out" (带下划线), 不是 "timedout"。

### 2.4 PhaseSpanSummary 与 StartedSpanInfo

Phase span 跟踪嵌套执行单元:
- PhaseSpanStarted 事件到达时 -> 存入 started_spans (临时)
- PhaseSpanDone 事件到达时 -> 合并 started_spans 中的 planned 和 started_at, 移入 completed_spans

## 3. 写入逻辑

Checkpoint 有 **5 条写入路径**，分布在 `RunStore` 和 `JournalStore` 两层。理解每条路径的触发条件、锁行为和原子性保证是排查数据一致性问题的前提。

### 3.1 写入路径总览

```
                        ┌─────────────────────────────────────────────────┐
                        │              checkpoint.json (磁盘)              │
                        └─────────────────────────────────────────────────┘
                          ▲          ▲          ▲          ▲          ▲
                          │          │          │          │          │
              ┌───────────┘    ┌─────┘    ┌─────┘    ┌─────┘    ┌─────┘
              │                │          │          │          │
     ① init_run()     ② append_event()  ③ upsert   ④ save    ⑤ cancel
     init_run_meta()  -> update_from_     _agent_    _check     ()
                      event()             _result()  _point()
                      -> write_check
                         _point_to_disk()

     原子性: ①②④        temp+rename      ③⑤ 直接 fs::write (非原子)
     锁:     ①④ 自取写锁   ② 已持有写锁     ③ 自取写锁   ⑤ 自取写锁
```

### 3.2 路径 ①: 初始化写入 (init_run / init_run_with_meta)

**触发**: 新建 run 时, `prepare()` 调用 `journal.init_run()` -> `RunStore::init_run()`

**流程** (state.rs:187):
1. 构造初始 `RunCheckpoint` (status=Running, 所有集合为空)
2. 调用 `save_checkpoint()` -> temp+rename 原子写入 `checkpoint.json`
3. 以 `create+append` 模式打开 `events.jsonl`
4. 将 checkpoint 存入内存 `RwLock<Option<RunCheckpoint>>`
5. 将 events file 存入内存 `RwLock<Option<File>>`

**写入内容**: 完整的初始 checkpoint JSON (pretty-print)

**原子性**: 是 (通过 `save_checkpoint` -> temp+rename)

### 3.3 路径 ②: 事件驱动写入 (append_event -> update_from_event -> write_checkpoint_to_disk)

**触发**: 每当 `AgentEvent` 到达 `RunStore::append_event()` 时自动执行。这是运行期间最频繁的写入路径。

**流程** (state.rs:304 -> 321 -> 446):

```
append_event(event)
  │
  ├─ 1. serde_json::to_string(event)
  ├─ 2. writeln!(events_file, json) + flush()     ← events.jsonl 追加写
  │
  └─ 3. update_from_event(event)                   ← 内存 checkpoint 增量更新
       │
       ├─ 3a. 获取 checkpoint.write() 锁
       ├─ 3b. event_stats[type_name] += 1
       ├─ 3c. match event 类型:
       │     AgentStarted  -> push started_agent_ids
       │     AgentDone     -> upsert agent_results (保留已有字段), total_tokens +=
       │     PhaseDone     -> update current_phase
       │     PhaseSpanStarted -> push started_spans
       │     PhaseSpanDone    -> merge -> completed_spans, remove from started_spans
       │     RunDone       -> set status, total_tokens, report
       │     _             -> noop
       ├─ 3d. checkpoint.updated_at = now()
       │
       └─ 3e. write_checkpoint_to_disk(checkpoint)  ← 原子落盘
              ├─ serde_json::to_string_pretty(cp)
              ├─ fs::write("checkpoint.json.tmp", content)
              └─ fs::rename(".tmp" -> "checkpoint.json")   ← 原子替换
```

**锁行为**: `update_from_event` 在入口获取 `checkpoint.write()` 锁, 持有锁直到 `write_checkpoint_to_disk` 完成。`write_checkpoint_to_disk` 本身 **不再获取锁** (private 方法, 调用方已持有)。

**原子性**: 是 (temp + rename)

**AgentDone 的字段保留逻辑** (state.rs:341-361):

`update_from_event` 处理 `AgentDone` 时, 不是直接用事件中的字段覆盖, 而是先查 `checkpoint.agent_results` 中已有的条目:

| 字段 | 来源 | 原因 |
|------|------|------|
| `cache_key_hash` | existing (保留) | `record_result()` 先写入 hash, 不能被 `None` 覆盖 |
| `output` | existing (保留) | `record_result()` 写入结构化 output, 事件中的 output 可能是 Null |
| `findings` | existing (保留) | 同上 |
| `phase_id` | existing (保留) | 事件不携带 phase_id |
| `completed_at` | existing (保留) | 保持首次写入的时间戳 |
| `description` / `role` | existing (保留) | `record_result()` 写入的元数据 |
| `status` | event (覆盖) | scheduler 回调的 status 更权威 |
| `tokens` | event (覆盖) | 事件携带的 token 计数 |
| `elapsed_ms` | event (覆盖) | 事件携带的耗时 |
| `name` | event (覆盖) | 事件携带的 agent 名称 |

### 3.4 路径 ③: 直接 upsert 写入 (upsert_agent_result)

**触发**: `JournalStore` 的三个方法调用:
- `cache_agent()` (journal.rs:250) - scheduler 回调路径
- `record_result()` (journal.rs:322) - Lua SDK `agent()` 函数
- `on_agent_done()` 回调 (journal.rs:473) - scheduler 完成后回调

**流程** (state.rs:169):
```
upsert_agent_result(cache)
  │
  ├─ 1. 获取 checkpoint.write() 锁
  ├─ 2. checkpoint.agent_results.insert(agent_id, cache)
  ├─ 3. checkpoint.updated_at = now()
  ├─ 4. clone checkpoint
  ├─ 5. 释放写锁
  │
  └─ 6. fs::write("checkpoint.json", content)   ← 直接写入, 非原子!
```

**与路径 ② 的关键区别**:
- **不追加 events.jsonl** - 只有 checkpoint.json 被更新
- **不更新 total_tokens** - 避免 token 双重计算
- **不更新 event_stats** - 不是事件驱动路径
- **非原子写入** - 直接 `fs::write`, 不使用 temp+rename

> **注意**: `upsert_agent_result` 使用 `fs::write` 直接覆写, 而非 temp+rename。如果在写入过程中进程崩溃, `checkpoint.json` 可能被截断。这是一个已知的权衡: 此路径不追加事件日志, 所以即使 checkpoint 损坏, `events.jsonl` 仍然完整, `resolve_resume` 可以从事件日志重建状态。但在当前实现中, resolve_resume 依赖 `checkpoint.json` 而非事件回放, 所以截断的 checkpoint 会导致 resume 失败。

**为什么不用 temp+rename**: `upsert_agent_result` 在释放写锁后才执行 `fs::write`, 如果用 temp+rename 需要在锁内完成所有操作或引入额外同步。当前实现选择了简单性 over 原子性, 依赖 `events.jsonl` 作为恢复源。

### 3.5 路径 ④: 公开 API 写入 (save_checkpoint)

**触发**: 仅由 `init_run` / `init_run_with_meta` 调用 (内部使用)。外部代码不直接调用。

**流程** (state.rs:457):
```
save_checkpoint(checkpoint)
  │
  ├─ 1. serde_json::to_string_pretty(checkpoint)
  ├─ 2. fs::write("checkpoint.json.tmp", content)    ← temp 文件
  ├─ 3. fs::rename(".tmp" -> "checkpoint.json")      ← 原子替换
  │
  └─ 4. 获取 checkpoint.write() 锁, 存入内存
```

**与路径 ② 的区别**: `save_checkpoint` 先写磁盘再更新内存; `write_checkpoint_to_disk` 在已持有锁的情况下只写磁盘 (内存已在调用方更新)。

**原子性**: 是 (temp + rename)

### 3.6 路径 ⑤: 取消写入 (cancel)

**触发**: 用户调用 `luft cancel` 或 MCP server 调用 `cancel_run()`

**流程** (state.rs:525):
```
cancel()
  │
  ├─ 1. 获取 checkpoint.write() 锁
  │
  ├─ 2. 如果内存 checkpoint 为 None (跨进程场景):
  │     ├─ 读 checkpoint.json
  │     ├─ 反序列化为 RunCheckpoint
  │     └─ 存入内存
  │
  ├─ 3. checkpoint.status = Cancelled
  ├─ 4. checkpoint.updated_at = now()
  ├─ 5. 释放写锁
  │
  ├─ 6. 获取 checkpoint.read() 锁
  └─ 7. fs::write("checkpoint.json", content)   ← 直接写入, 非原子!
```

**跨进程安全**: 当 MCP server 进程调用 `cancel()` 时, 内存缓存为空 (因为 `init_run` 在 `luft run` 进程中执行)。`cancel()` 会从磁盘加载 checkpoint, 修改 status, 再写回。

**非原子写入**: 与 `upsert_agent_result` 相同, 使用 `fs::write` 直接覆写。

**幂等性**: 多次调用 `cancel()` 安全。第二次调用时 status 已是 Cancelled, 不会产生额外副作用。

### 3.7 写入路径对比

| 路径 | 方法 | 原子性 | 写 events.jsonl | 更新 total_tokens | 锁获取 |
|------|------|--------|-----------------|-------------------|--------|
| ① 初始化 | init_run | temp+rename | No (仅创建文件) | No | 自取写锁 |
| ② 事件驱动 | append_event | temp+rename | Yes (追加) | Yes | 已持有写锁 |
| ③ 直接 upsert | upsert_agent_result | fs::write | No | No | 自取写锁 |
| ④ 公开 API | save_checkpoint | temp+rename | No | No | 自取写锁 |
| ⑤ 取消 | cancel | fs::write | No | No | 自取写锁 |

### 3.8 双写问题与设计决策

`cache_agent()` 路径会触发 **两次磁盘写入**:

```
cache_agent(key, agent_id, ...)
  │
  ├─ upsert_agent_result(cache)           ← 写入 ③: checkpoint.json (含 cache_key_hash)
  │
  └─ append_event(AgentDone)
       └─ update_from_event()
            └─ write_checkpoint_to_disk() ← 写入 ②: checkpoint.json (保留 hash)
```

**为什么不合并**: 两次写入服务于不同目的:
- 写入 ③ 确保 `cache_key_hash` 立即持久化 (防止进程崩溃后 hash 丢失)
- 写入 ② 确保事件被追加到 `events.jsonl` + 更新 `total_tokens` + `event_stats`

**字段保留机制防止数据丢失**: 写入 ② 的 `update_from_event` 会先检查 `checkpoint.agent_results` 中已有的条目。如果写入 ③ 已存入 `cache_key_hash`, 写入 ② 不会覆盖为 `None` (state.rs:356: `existing.and_then(|c| c.cache_key_hash.clone())`)。

`record_result()` 只触发一次写入 (路径 ③), 不追加事件。后续 scheduler 的 `on_agent_done` 回调可能再触发一次 `upsert_agent_result` (路径 ③) + 一次 `append_event` (路径 ②)。`on_agent_done` 同样保留已有 `cache_key_hash` (journal.rs:455: `existing.as_ref().and_then(|c| c.cache_key_hash.clone())`)。

## 4. 持久化引擎: RunStore (state.rs)

### 4.1 结构

RunStore 包含:
- run_dir: PathBuf
- checkpoint: RwLock<Option<RunCheckpoint>> - 内存缓存
- events_file: RwLock<Option<File>> - events.jsonl 句柄

RunStore 通过全局 DashMap 索引 (get_run_store()), 同一进程内同一 run 目录共享实例。

### 4.2 关键方法

- init_run(run_id, task) - 创建 run 目录, 写入初始 checkpoint.json, 打开 events.jsonl
- init_run_with_meta(run_id, task, meta) - 同上, 附加 workflow_meta
- open_run(run_id) - 从磁盘加载 checkpoint.json, 以 append 模式打开 events.jsonl
- append_event(event) - 追加到 events.jsonl + 调用 update_from_event()
- update_from_event(event) - 核心: 增量更新内存 checkpoint 并落盘
- write_checkpoint_to_disk(cp) - 原子写入 (checkpoint.json.tmp -> rename)
- save_checkpoint(cp) - 公开 API: 更新内存 + 落盘
- upsert_agent_result(cache) - 直接插入/更新 agent 结果 (JournalStore 使用)
- cancel() - 设置 Cancelled 状态并落盘; 支持跨进程 (从磁盘加载)
- can_resume() - 检查 status == Running
- get_checkpoint() - 返回内存 checkpoint 的克隆
- get_event_log() - 从 events.jsonl 回放全部事件

### 4.3 原子写入

write_checkpoint_to_disk() 使用 temp + rename 模式:
1. 写入 checkpoint.json.tmp
2. rename 为 checkpoint.json (原子操作)

同一文件系统上 rename 是原子的, 崩溃时不会产生部分写入。

### 4.4 事件 -> Checkpoint 更新流

update_from_event() 处理每种事件类型:
1. event_stats[type_name] += 1 (所有事件)
2. AgentStarted -> push to started_agent_ids
3. AgentDone -> upsert agent_results, total_tokens += tokens
4. PhaseDone -> update current_phase
5. PhaseSpanStarted -> push to started_spans
6. PhaseSpanDone -> merge started_spans -> completed_spans
7. RunDone -> set status, total_tokens, report
8. write_checkpoint_to_disk() (原子落盘)

重要: AgentDone 处理器会保留已有 AgentResultCache 中的 cache_key_hash, output, findings 等字段。这防止 scheduler 回调覆盖 cache key。

## 5. Journal 层: JournalStore (journal.rs)

### 5.1 结构

JournalStore 包含:
- inner: Arc<RunStore> - 底层持久化
- cache_index: RwLock<HashMap<String, AgentResultCache>> - O(1) 查找索引
- event_tx: Option<EventSender> - 事件广播

cache_index 同时以 cache_key_hash 和 agent_id.to_string() 为 key 建索引, 使 has_completed(key) 和 get_cached(key) 均为 O(1)。

### 5.2 关键方法

- new(run_dir) - 创建 RunStore, 空 cache_index
- init_run(run_id, task) - 委托 RunStore::init_run
- init_run_with_meta(run_id, task, meta) - 委托 RunStore::init_run_with_meta
- open(run_id) - Resume 入口: 加载 checkpoint + 重建 cache_index
- cache_agent(key, ...) - 持久化 agent 结果 + 更新索引 + 广播事件
- record_result(key, ...) - 持久化 agent 结果 + 更新索引 (不追加事件, 不重复计 token)
- has_completed(key) - O(1) 判断是否可跳过
- get_cached(key) - O(1) 获取缓存结果
- store() - 返回底层 Arc<RunStore>
- cancel() - 委托 RunStore::cancel
- flush() - 空操作 (RunStore 每次事件自动落盘)

### 5.3 cache_agent vs record_result

| | cache_agent() | record_result() |
|---|---|---|
| 写 checkpoint | upsert_agent_result | upsert_agent_result |
| 更新内存索引 | Yes | Yes |
| 追加 AgentDone 事件 | Yes | No |
| 累加 total_tokens | Yes | No |
| 广播事件 | Yes | No |
| 调用方 | scheduler 回调路径 | Lua SDK agent() 函数 |

设计原因: Lua SDK 在 agent 完成后先调用 record_result() 写入结构化结果 (含 cache_key_hash), 然后 scheduler 的 on_agent_done 回调触发 append_event(AgentDone)。如果 cache_agent 也追加事件, 会导致 token 双重计算。

### 5.4 on_agent_done 回调

JournalStore 实现 JournalCallback trait, scheduler 在每个 agent 完成后调用。

关键逻辑: 保留已有 cache_key_hash。如果 record_result() 先写入了 hash, on_agent_done 不能用 None 覆盖它, 否则 resume 时 agent 会被重复执行。

### 5.5 AgentCacheKey

确定性去重键, 用于 --resume 时跳过已完成的 agent 调用:
- hash: String - blake3 hex
- prompt_preview: String - 前 80 字符 (人类可读)
- phase_id: PhaseId

使用 blake3, \0 分隔符防止字段拼接冲突。prompt 经过空白折叠 + 换行统一归一化。相同 prompt + phase 产生相同 hash, resume 时用于匹配。

## 6. Resume 流程

### 6.1 概览

luft run --resume <run_dir_name> 的完整路径:
1. check_resumable(dir_name, base_dir) - 只读 status 字段, 返回 CanResume/NotFound/NotResumable
2. resolve_resume(dir_name, base_dir) - 读 checkpoint.json + workflow.lua, 返回 RunSpec
3. prepare(spec, ...) - 构建运行时: JournalStore::open(run_id), 注入 completed_spans, 启动事件转发器
4. execute(run_ctx, runtime, script) - 在阻塞线程上执行 Lua, Lua SDK agent() 调用 has_completed(key)

### 6.2 check_resumable (run.rs)

轻量级只读检查, 不加载完整 checkpoint:
- 读 checkpoint.json, 只解析 status 字段
- Completed/Cancelled/Failed -> NotResumable(status)
- Running 或 JSON 损坏或文件不存在 -> CanResume (宽松策略)

### 6.3 resolve_resume (run.rs)

完整恢复:
1. 读取并反序列化 checkpoint.json
2. 检查 status 必须为 Running, 否则 bail
3. 读取 workflow.lua (必须存在)
4. 从 checkpoint.workflow_meta 恢复 PlanMeta
5. 返回 RunSpec { resuming: true, ... }

### 6.4 prepare 中的 resume 逻辑

如果 resuming: journal.open(spec.run_id) 加载 checkpoint + 重建索引
否则: 写入 workflow.lua + journal.init_run() 初始化 checkpoint

resume 时注入已完成的 phase spans: 从 checkpoint.completed_spans 提取 name 列表, 调用 runtime.set_completed_spans()

### 6.5 RunCreationMode (journal.rs)

| 模式 | 行为 |
|------|------|
| New { task } | 生成新 RunId, 不加载 checkpoint |
| Resume { run_id, run_dir_name } | 打开指定 run 的 JournalStore, 加载 checkpoint |
| Auto { task } | 扫描 journal 目录, 找最新的 Running checkpoint; 找不到则新建 |

### 6.6 latest_resumable 与 find_resumable_by_task

- latest_resumable(base_dir) - --resume 无参数时找最新有 checkpoint.json 的 run
- find_resumable_by_task(task, base_dir) - 按 task 名称匹配, 返回最新的 Running run

## 7. 查询层 (query.rs)

跨进程安全的只读查询, 直接读磁盘不依赖内存缓存。

### 7.1 StatusOutput DTO

从 RunCheckpoint 投影的查询 DTO:
- run_id, run_dir, task, status: String
- current_phase: u32
- completed_phases: usize
- total_started: usize (started_agent_ids.len())
- completed_agents: usize (agent_results.len())
- running_agents: usize (started - done)
- total_tokens: u64
- created_at, updated_at: String (RFC3339)

### 7.2 查询函数

| 函数 | 数据源 | 说明 |
|------|--------|------|
| get_checkpoint(dir, base) | 磁盘 | 读 checkpoint.json, 不查内存缓存 |
| get_status(dir, base) | 磁盘 | 投影为 StatusOutput |
| list_runs(base) | 磁盘 | 列出所有 run, 按 updated_at 降序 |
| get_report(dir, base) | 磁盘 | 快速路径: 读 checkpoint.report; 回退: 扫描 events.jsonl |
| get_findings(dir, base) | 磁盘 | 读 checkpoint.findings |
| cancel_run(dir, base) | 内存/磁盘 | 通过 get_run_store() 获取 RunStore 并调用 cancel() |
| get_events(dir, base) | 磁盘 | 回放 events.jsonl |
| get_logs(dir, base, limit) | 磁盘 | get_events() + JSON 序列化, 可限条数 |

跨进程安全: 所有查询函数直接读磁盘 checkpoint.json, 不依赖 RunStore 内存缓存。这使得 MCP server 可以查询由 luft run 创建的 run, 反之亦然。

## 8. 垃圾回收

gc_runs(journal_dir, older_than) (journal.rs):
- 扫描所有 run 目录
- Completed/Cancelled/Failed 且 updated_at 早于 cutoff 的 run 被删除
- Running 状态的 run 永不清理
- 返回清理数量

## 9. 已知局限

- cache key 无版本号 - 算法变更会导致已有缓存失效, 当前无迁移机制
- events.jsonl 无大小限制 - 长时间运行的 run 会产生大文件
- event_stats 在 resume 后不重置 - 继续累加
