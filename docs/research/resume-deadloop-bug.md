# Resume 死循环：PhaseDone 未发射 + 失败 agent 无缓存

> **状态**：已定位，待修复
> **发现来源**：`luft-workflow_1785042726` 实例（61 endpoint × 306 agent conformance audit）
> **影响范围**：所有使用 `--resume` 的多 phase 工作流
> **交叉参考**：[../architecture/runtime.md](../architecture/runtime.md) §4 SDK 原语、[../architecture/core.md](../architecture/core.md) §4 state 持久化

---

## 1. 症状

一个 306-agent 的 conformance audit workflow（3 phase：audit → adversary → doc-write）在 Phase 1 部分完成后被中断。用户执行 `--resume`，观察到：

- **LLM 一直在被请求**——每次 resume 都重新调用 LLM
- **没有可见进度**——`current_phase` 始终为 0，CLI 进度显示停在 "phase 0"
- **反复 resume 无法收敛**——已完成的 agent 被跳过（cache 命中），但失败的 agent 每次 resume 都重跑

### 1.1 实例证据

```
checkpoint.json:
  "current_phase": 0,          ← 应为 1 或更高
  "completed_phases": [],      ← 应包含已完成的 phase

events.jsonl:
  rg -c "run_started"   -> 5   ← 1 次初始 + 4 次 resume
  rg -c "phase_done"    -> 0   ← 零条 PhaseDone 事件
  rg -c "phase_started" -> 8   ← 每次 resume 重新发射（3 phase × 多轮 resume）
  rg -c "agent_done.*Error" -> 352
  rg -c "agent_done.*Ok"     -> 224
```

checkpoint 中 4 个 error agent 的 `phase_id` 均为 `0`（实际属于 phase 1），`cache_key_hash` 均为 `null`。首次运行即有 11 个 audit agent 超时失败（audit-11 ~ audit-21）。

---

## 2. 根因分析

共发现 4 个 bug，分主次排列。

### 2.1 Bug 1（主因）：`PhaseDone` 事件从未被发射

**位置**：`crates/luft-runtime/src/sdk/control.rs:84`

`phase()` Lua 原语只发射 `PhaseStarted`，整个 runtime 中没有任何代码路径发射 `PhaseDone`：

```rust
// control.rs:84 — phase() 函数体内
let _ = events.send(AgentEvent::PhaseStarted {   // ✅ 有
    run_id, phase_id, label, planned, ...
});
// ❌ 没有对应的 PhaseDone 发射
```

`parallel()` 发射的是 `ParallelStarted` / `ParallelDone`，不是 `PhaseDone`。`phase_begin()` / `phase_end()` 发射的是 `PhaseSpanStarted` / `PhaseSpanDone`（结构化 span），也不是 `PhaseDone`。

**影响链**：

```
PhaseDone 从未发射
  → state.rs:315-318 的 PhaseDone handler 从未触发
  → checkpoint.current_phase 永远保持初始值 0
  → CLI 进度显示始终 "phase 0"
  → checkpoint 无法记录 "中断时处于哪个 phase"
```

### 2.2 Bug 2：`AgentStarted` 丢弃 `phase_id`，`AgentDone` 不携带它

**位置**：`crates/luft-core/src/state.rs:310` + `crates/luft-core/src/contract/event.rs:72-92`

`AgentStarted` 事件**包含** `phase_id` 字段（event.rs:38），但 `update_from_event` handler 用 `..` 将其忽略：

```rust
// state.rs:310
AgentEvent::AgentStarted { agent_id, .. } => {  // phase_id 被 .. 丢弃
    if !checkpoint.started_agent_ids.contains(agent_id) {
        checkpoint.started_agent_ids.push(*agent_id);
    }
}
```

`AgentDone` 事件**不包含** `phase_id` 字段（event.rs:72-92），handler 从 `agent_results` 中查找已有记录：

```rust
// state.rs:293
phase_id: existing.map(|c| c.phase_id).unwrap_or(0),  // 新 agent → 0
```

对新 agent（无已有记录），`phase_id` 默认为 `0`。

**影响**：checkpoint 中 error agent 的 `phase_id` 为 `0` 而非实际 phase。成功 agent 的 `phase_id` 正确，因为 `parallel.rs:85` 的 `record()` 函数直接写入了正确的 `phase_id`。

### 2.3 Bug 3：失败 agent 不被缓存

**位置**：`crates/luft-runtime/src/sdk/agent/parallel.rs:81-94`

`parallel()` 中 `record()` 只在成功路径调用：

```rust
// parallel.rs:81-94
Ok(r) => {
    record(&journal, &p.cache_key, p.agent_id, phase_id, &r);  // ✅ 缓存
    slot_from_result(r)
}
Err(e) => {
    // ❌ 不调用 record()，不缓存
    ("error".to_string(), json!({ "error": e.to_string() }), 0, vec![])
}
```

失败 agent 在 checkpoint 中 `cache_key_hash` 为 `null`。Resume 时 `journal.get_cached()` 查不到缓存，agent 被重新提交到 scheduler → 重新调用 LLM → 再次失败/超时。

**影响**：这是"一直在请求 LLM"的直接原因。实例中 7 个 `integration.*` audit agent 在首次运行时全部超时（~600s，0 token），每次 resume 都重跑。

### 2.4 Bug 4：`completed_phases` 从不被更新

**位置**：`crates/luft-core/src/state.rs:315-318`

即使 `PhaseDone` 被发射，handler 也只更新 `current_phase`，不向 `completed_phases` 推送：

```rust
// state.rs:315-318
AgentEvent::PhaseDone { phase_id, .. } => {
    if *phase_id > 0 {
        checkpoint.current_phase = *phase_id;  // ✅ 更新 current_phase
        // ❌ 不更新 completed_phases
    }
}
```

---

## 3. Bug 间因果关系

```
                    ┌──────────────────────────────────────┐
                    │         首次运行：11 个 agent 超时       │
                    │         运行被中断                      │
                    └──────────────┬───────────────────────┘
                                   │
                    ┌──────────────▼───────────────────────┐
                    │  Bug 1: PhaseDone 从未发射             │
                    │  -> current_phase = 0                  │
                    │  -> checkpoint 无 phase 进度信息        │
                    └──────────────┬───────────────────────┘
                                   │
                    ┌──────────────▼───────────────────────┐
                    │         用户执行 --resume              │
                    └──────────────┬───────────────────────┘
                                   │
              ┌────────────────────┼────────────────────┐
              │                    │                    │
   ┌──────────▼─────────┐ ┌────────▼─────────┐ ┌────────▼──────────┐
   │ Bug 3: 失败 agent   │ │ 成功 agent        │ │ Bug 1: 进度显示    │
   │ 无缓存 -> 重跑       │ │ cache 命中 -> 跳过  │ │ current_phase=0   │
   │ -> 请求 LLM          │ │ -> 快速返回         │ │ -> "没有进度"      │
   │ -> 再次超时/失败     │ └────────┬─────────┘ └────────┬──────────┘
   └──────────┬─────────┘          │                    │
              │                    │                    │
              └────────────────────┼────────────────────┘
                                   │
                    ┌──────────────▼───────────────────────┐
                    │  用户观察到：LLM 在请求，但没进度       │
                    │  -> 中断 -> 再次 resume -> 死循环       │
                    │  （实际发生 5 轮，累积 352 次 Error）     │
                    └──────────────────────────────────────┘
```

---

## 4. 实例数据佐证

### 4.1 checkpoint.json 关键字段

```json
{
  "current_phase": 0,           // Bug 1: 应为 1+
  "completed_phases": [],       // Bug 4: 应包含已完成 phase
  "agent_results": {
    "019f9e56-...81427": {      // error agent
      "phase_id": 0,            // Bug 2: 应为 1
      "status": "error",
      "cache_key_hash": null,   // Bug 3: 失败 agent 无缓存
      "tokens": 0
    },
    "019f9d5f-...7150": {       // success agent
      "phase_id": 1,            // 正确（record() 直接写入）
      "status": "ok",
      "cache_key_hash": "b0cc...",  // 正确（record() 写入）
      "tokens": 2003651
    }
  }
}
```

### 4.2 events.jsonl 统计

| 事件类型 | 总数 | 说明 |
|----------|------|------|
| `run_started` | 5 | 1 次初始运行 + 4 次 resume |
| `phase_started` | 8 | 每次 resume 重新发射（3 phase × 多轮 resume，部分 phase 未触达） |
| `phase_done` | **0** | **Bug 1：从未发射** |
| `agent_started` | 593 | 跨 5 轮运行累积 |
| `agent_done` (Ok) | 224 | 部分为 cache 命中快速返回 |
| `agent_done` (Error) | 352 | **Bug 3：无缓存，每次 resume 重跑后再次失败** |

### 4.3 首次运行失败 agent

首次运行（第一次 `run_started` 到第二次 `run_started` 之间）有 11 个 audit agent 全部超时，0 token：

| Agent | Endpoint | elapsed_ms | 状态 |
|-------|----------|------------|------|
| audit-11 | integration.get | 691,244 | Error |
| audit-12 | integration.connect.key | 622,451 | Error |
| audit-13 | integration.connect.oauth | 606,248 | Error |
| audit-14 | integration.attempt.status | 586,582 | Error |
| audit-15 | integration.attempt.complete | 583,138 | Error |
| audit-16 | integration.attempt.cancel | 584,943 | Error |
| audit-17 | location.get | 586,275 | Error |
| audit-18 | session.messages | 642,996 | Error |
| audit-19 | model.list | 643,198 | Error |
| audit-20 | permission.request.list | 661,460 | Error |
| audit-21 | permission.saved.list | 695,839 | Error |

每次 resume 这 11 个 agent 都被重新提交到 scheduler，每个耗时 ~600s，单轮总计 ~115 分钟无进展。5 轮 resume 累积产生 352 次 Error（含 adversary/doc 阶段的级联失败）。

---

## 5. 修复方案

### 5.1 Bug 1 修复：发射 PhaseDone

**方案**：在 `phase()` 函数中，当检测到前一个 phase 存在时，先发射前一个 phase 的 `PhaseDone`。

**文件**：`crates/luft-runtime/src/sdk/control.rs`

```rust
// phase() 函数内，在发射 PhaseStarted 之前：
let prev_phase_id = phase_counter.load(Ordering::Relaxed);
if prev_phase_id > 0 {
    // 统计前一 phase 的 ok/failed（需新增 helper，从 journal 的 agent_results 中按 phase_id 聚合）
    let (ok, failed) = count_phase_results(&journal, prev_phase_id);
    let _ = events.send(AgentEvent::PhaseDone {
        run_id,
        phase_id: prev_phase_id,
        ok,
        failed,
        ts: chrono::Utc::now(),
    });
}
```

> **注意**：`count_phase_results` 是需要新增的 helper 函数，从 `JournalStore` 的 `agent_results` 中按 `phase_id` 聚合 ok/failed 计数。`phase()` 的 Lua 闭包当前未捕获 `journal`，需要额外注入。

**补充**：在 `Runtime::execute()` 结束时（脚本执行完毕），也需为最后一个 phase 发射 `PhaseDone`。

### 5.2 Bug 2 修复：记录 agent 的 phase_id

**方案 A（推荐）**：在 `AgentStarted` handler 中记录 `phase_id`。

**文件**：`crates/luft-core/src/state.rs:310`

```rust
AgentEvent::AgentStarted { agent_id, phase_id, .. } => {
    if !checkpoint.started_agent_ids.contains(agent_id) {
        checkpoint.started_agent_ids.push(*agent_id);
    }
    // 预创建 agent_results 条目，记录 phase_id
    checkpoint.agent_results.entry(*agent_id).or_insert(AgentResultCache {
        agent_id: *agent_id,
        phase_id: *phase_id,
        status: "running".to_string(),
        output: serde_json::Value::Null,
        findings: vec![],
        tokens: 0,
        completed_at: current_timestamp(),
        cache_key_hash: None,
        description: None,
        role: None,
    });
}
```

**方案 B**：给 `AgentDone` 事件添加 `phase_id` 字段（需修改冻结合约，不推荐）。

### 5.3 Bug 3 修复：缓存失败 agent

**方案**：在 `parallel()` 的 `Err` 分支中也调用 `record()`，但用 error 结果。

**文件**：`crates/luft-runtime/src/sdk/agent/parallel.rs:88-92`

```rust
Err(e) => {
    let err_result = AgentResult {
        agent_id: p.agent_id,
        status: AgentStatus::Error,
        output: serde_json::json!({ "error": e.to_string() }),
        findings: vec![],
        tokens_used: TokenUsage::default(),
        artifacts: vec![],
        logs: Default::default(),
        thread_id: None,
    };
    record(&journal, &p.cache_key, p.agent_id, phase_id, &err_result);
    slot_from_result(err_result)
}
```

**权衡**：缓存失败结果意味着 resume 后不会重试。需配合 `run_fanout` 的重试逻辑——如果 Lua 脚本本身已有 3 次重试，则缓存失败结果是合理的；如果脚本期望 resume 后重试失败 agent，则不应缓存。

**推荐**：缓存失败结果 + 在 `AgentResultCache` 中增加 `retry_count` 字段。Resume 时若 `retry_count < max_retries` 则重试，否则跳过。

### 5.4 Bug 4 修复：更新 completed_phases

**文件**：`crates/luft-core/src/state.rs:315-318`

```rust
AgentEvent::PhaseDone { phase_id, ok, failed, .. } => {
    if *phase_id > 0 {
        checkpoint.current_phase = *phase_id;
        checkpoint.completed_phases.push(PhaseSummary {
            phase_id: *phase_id,
            ok: *ok,
            failed: *failed,
            // ...
        });
    }
}
```

---

## 6. 改动文件清单

| 文件 | 改动类型 | Bug | 说明 |
|------|----------|-----|------|
| `crates/luft-runtime/src/sdk/control.rs` | 修改 | 1 | `phase()` 发射前一个 phase 的 `PhaseDone` |
| `crates/luft-runtime/src/sandbox.rs` | 修改 | 1 | `execute()` 结束时发射最后一个 phase 的 `PhaseDone` |
| `crates/luft-core/src/state.rs` | 修改 | 2, 4 | `AgentStarted` 记录 `phase_id`；`PhaseDone` 更新 `completed_phases` |
| `crates/luft-runtime/src/sdk/agent/parallel.rs` | 修改 | 3 | `Err` 分支调用 `record()` 缓存失败结果 |
| `crates/luft-runtime/src/sdk/agent/single.rs` | 修改 | 3 | 同上（`agent()` 单 agent 路径） |

---

## 7. 测试计划

| 测试 | 验证点 | Bug |
|------|--------|-----|
| `phase_done_emitted_on_next_phase` | 调用 `phase("a")` 后 `phase("b")`，events 中有 phase 1 的 `PhaseDone` | 1 |
| `phase_done_emitted_on_script_end` | 脚本正常结束后，最后一个 phase 有 `PhaseDone` | 1 |
| `agent_started_records_phase_id` | `AgentStarted` 后 checkpoint 中 `agent_results[aid].phase_id` 正确 | 2 |
| `failed_agent_cached_on_resume` | agent 失败后 resume，`journal.get_cached()` 返回 error 结果 | 3 |
| `completed_phases_updated_on_phase_done` | `PhaseDone` 后 `checkpoint.completed_phases` 非空 | 4 |
| `resume_skips_failed_after_max_retry` | 失败 agent 缓存后，resume 不重跑（`retry_count` 达上限） | 3 |
| 端到端：多 phase workflow 中断 + resume | `current_phase` 正确、失败 agent 不无限重跑、进度显示正确 | 全部 |

---

## 8. 向后兼容性

- **`PhaseDone` 事件新增发射**：`PhaseDone` 变体已存在于 `AgentEvent` 枚举中（event.rs:93-100），所有消费者（state.rs、phases.rs、writer.rs、phase_renderer.rs）已有对应 handler，只是从未被触发。新增发射是**纯补充**，不破坏现有行为。
- **`AgentStarted` handler 预创建条目**：`or_insert` 语义保证不覆盖已有条目（`record()` 先写入的值不会被覆盖）。
- **失败 agent 缓存**：新增行为。旧的 checkpoint（无缓存）在 resume 时仍会重跑失败 agent，但跑完后会被缓存，后续 resume 不再重跑。无迁移需求。
- **`completed_phases` 更新**：纯增量，不影响已有读取逻辑（`phases.rs` 已处理 `completed_phases` 非空的情况）。
