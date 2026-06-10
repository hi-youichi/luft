# Maestro 日志系统审计

## 1. 覆盖率现状

| 模块 | 日志调用数 | 带 run_id | run_id 覆盖率 |
|------|----------|----------|-------------|
| `core/scheduler/mod.rs` | 7 | 5 | 71% |
| `runtime/converge.rs` | 6 | 6 | 100% |
| `runtime/pipeline.rs` | 1 | 1 | 100% |
| `adapters/acp_adapter.rs` | 3 | 2 | 67% |
| `runtime/sandbox.rs` | 3 | 0 | 0% |
| `core/journal.rs` | 5 | 0 | 0% |
| **总计** | **~29** | **14** | **48%** |

## 2. 已修复的关键盲区

| 路径 | 修复前 | 修复后 |
|------|--------|--------|
| Scheduler agent 成功 | 0 处日志 | `debug!("scheduler: agent succeeded")` + run_id |
| Scheduler agent 取消 | 0 处日志 | `warn!("scheduler: agent cancelled")` + run_id |
| Scheduler agent 超时 | 0 处日志 | `warn!("scheduler: agent timed out")` + run_id |
| Scheduler permit 取消 | 0 处日志 | `warn!("scheduler: cancelled before permit")` + run_id |
| Converge 收敛退出 | 0 处日志 | `info!("converged: ...")` + run_id |
| Converge 完成 | 0 处日志 | `info!("converge completed")` + rounds/findings/converged |

## 3. 仍存的盲区

| 路径 | 原因 |
|------|------|
| `Runtime::execute` 入口 | `Runtime` 结构体无 `run_id` 字段 |
| `Planner::plan_workflow` | 函数签名无 `run_id` 参数 |
| `Journal::flush/close` | 调用链未传递 `run_id` |

## 4. 日志级别约定

| 级别 | 用途 | 示例 |
|------|------|------|
| `ERROR` | 不可恢复错误 | Lua 执行失败、ACP 子进程崩溃 |
| `WARN` | 可恢复异常、重试、取消 | 语法验证失败、agent 重试、超时 |
| `INFO` | 关键状态转换 | 调度开始、收敛退出、pipeline 启动 |
| `DEBUG` | 详细信息 | agent 成功、journal 缓存命中、ACP PID |

## 5. 日志示例（正常重试链路）

```text
2026-06-07T10:04:07  INFO tracing initialized, log_file=maestro.log
2026-06-07T10:04:12  INFO no --backend specified, auto-detected: opencode
2026-06-07T10:04:13  INFO planning attempt attempt=1 max_retries=3
2026-06-07T10:04:13  INFO ACP session starting binary=opencode agent_id=b33c... prompt_len=3818 run_id=b32b...
2026-06-07T10:04:13 DEBUG ACP child spawned pid=28936
2026-06-07T10:04:26  WARN planner script validation failed attempt=1 error=lua syntax error: ...
2026-06-07T10:04:26  INFO ACP session starting binary=opencode agent_id=1721... prompt_len=4002 run_id=171e...
2026-06-07T10:04:26 DEBUG ACP child spawned pid=71624
```

`run_id` 串联 planner agent → scheduler agent → converge round，全链路可追踪。