# Resume Failed/Cancelled 工作流

> **状态**：已实现
> **依赖**：[../architecture/checkpoint.md](../architecture/checkpoint.md)、[../research/resume-deadloop-bug.md](../research/resume-deadloop-bug.md)
> **影响范围**：`check_resumable`、`resolve_resume`、`prepare`、Lua SDK `agent()` cache 逻辑、`find_resumable_by_task`

---

## 1. 背景与目标

### 1.1 现状

当前 Luft 只允许 `Running` 状态的 run 被 resume。`Failed` 和 `Cancelled` 被 `check_resumable()` 和 `resolve_resume()` 双重拦截：

```
check_resumable (run.rs:181-186)
  └─ matches!(status, Completed | Cancelled | Failed) → NotResumable

resolve_resume (run.rs:203-206)
  └─ matches!(checkpoint.status, Completed | Cancelled | Failed) → bail
```

用户遇到 failed/cancelled 的 run，只能从头新建 run 重跑全部 agent，即使大部分 agent 已成功完成。

### 1.2 目标

允许 `Failed` 和 `Cancelled` 状态的 run 被 resume：
- **已成功的 agent**：跳过（journal cache 命中）
- **失败的 agent**：重新执行
- **未启动的 agent**：正常执行
- **已完成的 run (Completed)**：仍不可 resume

### 1.3 与 resume-deadloop-bug 的关系

[resume-deadloop-bug.md](../research/resume-deadloop-bug.md) 修复的是 resume 机制本身的 4 个 bug（PhaseDone 未发射、phase_id 丢弃、parallel 失败 agent 未缓存、completed_phases 不更新）。那些 bug 修复后，resume 的基础机制才可靠。

本设计是在 bug 修复之上的**功能扩展**——放开终态门控，让 failed/cancelled 也能 resume。两者正交，可独立实现。

---

## 2. 设计方案

### 2.1 核心思路

Resume 的执行模型是**Lua 脚本从头重新执行**，靠 journal cache 跳过已完成 agent。因此不需要"从中间断点恢复"的复杂逻辑，只需要：

1. **放开门控**：让 `Failed`/`Cancelled` 状态通过 `check_resumable` 和 `resolve_resume`
2. **智能 cache 查询**：resume 时只跳过 `status == "ok"` 的缓存，非 ok 缓存视为 miss
3. **重置 checkpoint 状态**：resume 时将 `checkpoint.json` 的 status 改回 `running`
4. **放开辅助查找**：`find_resumable_by_task` 包含 failed/cancelled

### 2.2 Agent resume 行为矩阵

| Agent 上次状态 | journal cache | Resume 行为 |
|---|---|---|
| ok | cache hit, status="ok" | **跳过**（返回缓存结果） |
| error | cache hit, status="error" | **重新执行**（cache 被忽略） |
| timed_out | cache hit, status="timed_out" | **重新执行**（cache 被忽略） |
| cancelled | cache hit, status="cancelled" | **重新执行**（cache 被忽略） |
| 未启动 | cache miss | **正常执行** |

重新执行后，`record_result()` 会 upsert 覆盖旧的错误缓存（`journal.rs:308` 用 `HashMap::insert`）。

### 2.3 为什么不需要 retry_count

[resume-deadloop-bug.md](../research/resume-deadloop-bug.md) §5.3 曾建议在 `AgentResultCache` 中增加 `retry_count` 字段，配合 `max_retries` 控制重试次数。本设计**不采用**这一方案，理由：

- **Resume 是用户主动操作**，不是自动循环。用户看到 failed run 后决定 resume，期望的就是"重试所有失败 agent"。
- **死循环风险已在源头解决**：如果重试后仍然失败，checkpoint 再次变为 `failed`，用户需要再次手动 resume。不会自动触发。
- **简化实现**：不引入新的持久化字段，不修改 `AgentResultCache` 结构，无需序列化迁移。
- **如果未来需要自动重试上限**，可以在 agent 级别加 `retry_count` 字段作为增量改进，与当前设计兼容。

---

## 3. 改动详情

### 3.1 放开门控：`check_resumable`

**文件**：`crates/luft/src/run.rs:168-194`

```rust
// 之前：阻止 Completed | Cancelled | Failed
if matches!(
    status,
    CheckpointStatus::Completed
        | CheckpointStatus::Cancelled
        | CheckpointStatus::Failed
) {
    return ResumeCheck::NotResumable(status);
}

// 之后：只阻止 Completed
if matches!(status, CheckpointStatus::Completed) {
    return ResumeCheck::NotResumable(status);
}
```

### 3.2 放开门控：`resolve_resume`

**文件**：`crates/luft/src/run.rs:198-230`

```rust
// 之前：bail on Completed | Cancelled | Failed
if matches!(
    checkpoint.status,
    CheckpointStatus::Completed | CheckpointStatus::Cancelled | CheckpointStatus::Failed
) {
    anyhow::bail!("run {} is not resumable (status: {:?})", run_dir_name, checkpoint.status);
}

// 之后：只 bail on Completed
if matches!(checkpoint.status, CheckpointStatus::Completed) {
    anyhow::bail!("run {} is not resumable (status: completed)", run_dir_name);
}
```

### 3.3 智能 cache 查询：`single.rs`

**文件**：`crates/luft-runtime/src/sdk/agent/single.rs:39-52`

当前代码无条件返回任何缓存结果（包括 error）：

```rust
// 当前
if let Some(ref j) = journal {
    if let Some(cached) = j.get_cached(&cache_key) {
        // 直接返回，即使是 error 结果
        let (status, output, tokens, findings) = slot_from_cache(cached);
        return build_result_table(lua, &status, output, tokens, &findings);
    }
}
```

改为只跳过成功结果：

```rust
// 之后
if let Some(ref j) = journal {
    if let Some(cached) = j.get_cached(&cache_key) {
        if cached.status == "ok" {
            let _ = events.send(AgentEvent::Log {
                run_id,
                agent_id: None,
                level: LogLevel::Info,
                msg: format!(
                    "resume: skip cached agent ({}…)",
                    &cache_key.hash[..8.min(cache_key.hash.len())]
                ),
            });
            let (status, output, tokens, findings) = slot_from_cache(cached);
            return build_result_table(lua, &status, output, tokens, &findings);
        }
        // 非 ok 缓存：fall through，重新执行
    }
}
```

**对 parallel.rs 的影响**：`parallel.rs` 中的 `has_completed(key)` 检查也需要同样修改——只对 ok 状态返回 true。需要确认 `has_completed` 是否也检查 status。

### 3.4 重置 checkpoint 状态：`prepare`

**文件**：`crates/luft/src/run.rs:308-341`

Resume 时 `journal.open(run_id)` 之后，需要将 checkpoint 状态重置为 `running`：

```rust
if spec.resuming {
    journal
        .open(spec.run_id)
        .map_err(|e| anyhow::anyhow!("failed to open journal for resume: {}", e))?;

    // 将 checkpoint 状态重置为 running
    // RunDone 事件会在执行结束时自然设置最终状态
    journal.store().reset_status_to_running(spec.run_id);
}
```

**新增方法** `reset_status_to_running`（`crates/luft-core/src/state.rs`）：

```rust
pub fn reset_status_to_running(&self, _run_id: RunId) {
    let mut cp = self.checkpoint.write().unwrap();
    if let Some(checkpoint) = cp.as_mut() {
        checkpoint.status = CheckpointStatus::Running;
        checkpoint.updated_at = current_timestamp();
    }
    let cp_clone = cp.clone();
    drop(cp);
    if let Some(checkpoint) = cp_clone {
        let _ = write_checkpoint_to_disk(&self.run_dir, &checkpoint);
    }
}
```

> **注意**：`write_checkpoint_to_disk` 是 `RunStore` 的 private 方法。需要在 `state.rs` 中暴露一个 public 方法或复用 `save_checkpoint`。

**设计决策**：使用 temp+rename 原子写入（而非 `fs::write`），因为 resume 是低频操作，不需要为此牺牲原子性。

**为什么需要在 prepare 中重置而不是在 resolve_resume 中**：
- `resolve_resume` 是同步的纯函数，只负责构建 `RunSpec`
- `prepare` 是异步函数，持有 `JournalStore` 的引用，可以安全地修改 checkpoint
- 在 `RunStarted` 事件发出之前重置状态，保证整个执行期间 checkpoint 反映 `running`

### 3.5 放开辅助查找：`find_resumable_by_task`

**文件**：`crates/luft/src/run.rs:249-269`

```rust
// 之前：只匹配 Running
if cp.task == task && matches!(cp.status, CheckpointStatus::Running) {

// 之后：排除 Completed
if cp.task == task && !matches!(cp.status, CheckpointStatus::Completed) {
```

### 3.6 `latest_resumable` — 无需修改

`latest_resumable` 只检查 `checkpoint.json` 文件是否存在（`run.rs:239`），不检查 status。无需修改。

---

## 4. 改动文件清单

| 文件 | 改动类型 | 说明 |
|---|---|---|
| `crates/luft/src/run.rs` | 修改 | `check_resumable`、`resolve_resume`、`find_resumable_by_task` |
| `crates/luft-runtime/src/sdk/agent/single.rs` | 修改 | cache 查询只跳过 ok |
| `crates/luft-runtime/src/sdk/agent/parallel.rs` | 修改 | `has_completed` 只对 ok 返回 true |
| `crates/luft-core/src/state.rs` | 新增方法 | `reset_status_to_running` |
| `crates/luft/src/run.rs` `prepare()` | 修改 | resume 时调用 `reset_status_to_running` |

---

## 5. 测试计划

### 5.1 单元测试

| 测试 | 文件 | 验证点 |
|---|---|---|
| `check_resumable_allows_failed` | run.rs | Failed 状态 → `CanResume` |
| `check_resumable_allows_cancelled` | run.rs | Cancelled 状态 → `CanResume` |
| `check_resumable_blocks_completed` | run.rs | Completed 状态 → `NotResumable`（不变） |
| `resolve_resume_failed_succeeds` | run.rs | Failed checkpoint → 返回 RunSpec（不 bail） |
| `resolve_resume_cancelled_succeeds` | run.rs | Cancelled checkpoint → 返回 RunSpec |
| `resolve_resume_completed_bails` | run.rs | Completed checkpoint → bail（不变） |
| `resume_skips_ok_cache` | single.rs | ok 缓存 → 跳过 |
| `resume_retries_error_cache` | single.rs | error 缓存 → 重新执行 |
| `resume_retries_timed_out_cache` | single.rs | timed_out 缓存 → 重新执行 |
| `reset_status_to_running` | state.rs | Failed → Running，落盘 |
| `find_resumable_includes_failed` | run.rs | find_resumable_by_task 匹配 Failed run |

### 5.2 已有测试更新

以下已有测试的断言需要修改（从 `NotResumable` 改为 `CanResume`）：

| 测试 | 文件 | 修改 |
|---|---|---|
| `resume_check_failed` (run.rs:893) | run.rs | 改为 `CanResume` |
| `resume_check_cancelled` (run.rs:877) | run.rs | 改为 `CanResume` |
| `resolve_resume_not_resumable_failed` (run.rs:1081) | run.rs | 改为成功返回 RunSpec |

### 5.3 端到端测试

| 测试 | 验证点 |
|---|---|
| E2E-01: failed run resume | 3-agent workflow：agent A ok、agent B error、agent C 未启动 → resume → A 跳过、B+C 执行成功 → run completed |
| E2E-02: cancelled run resume | workflow 被取消 → resume → 已完成 agent 跳过、其余执行 |
| E2E-03: all-failed resume | 全部 agent 失败 → resume → 全部重新执行 |
| E2E-04: re-fail | failed run resume → 再次失败 → checkpoint 变回 failed，可再次 resume |

---

## 6. 向后兼容性

- **已有 failed/cancelled checkpoint 无需迁移**：resume 时自动重置为 running，journal cache 中的 ok 结果自然命中。
- **API 无变化**：`resume_from_id` 参数语义不变，只是不再拒绝 failed/cancelled。
- **MCP 工具无变化**：`workflow_start(resume_from_id=...)` 行为一致。
- **Completed 仍不可 resume**：行为不变。

---

## 7. 风险与缓解

### 7.1 失败 agent 可能再次失败

**风险**：如果 agent 失败原因是 LLM 不可达或 prompt 本身有误，resume 后会再次失败。

**缓解**：这是预期行为。Resume 是用户主动操作，用户应在 resume 前检查/修复失败原因（如 LLM 配置、网络等）。如果再次失败，checkpoint 回到 failed，用户可以修复后再次 resume。

### 7.2 token 成本

**风险**：重新执行失败 agent 会消耗 LLM token。

**缓解**：已成功的 agent 被跳过（cache 命中），只重跑失败部分。相比从头新建 run，节省了成功部分的 token。

### 7.3 partial() 中的 has_completed

**风险**：`parallel.rs` 使用 `has_completed(key)` 决定是否跳过。如果 `has_completed` 不检查 status，失败 agent 仍会被跳过。

**缓解**：需要同步修改 `has_completed` 或其在 parallel.rs 中的使用方式。详见 §3.3 的 parallel.rs 说明。
