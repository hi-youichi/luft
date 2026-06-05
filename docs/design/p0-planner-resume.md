# P0-B/C: Planner agent 化 & Resume — 实现设计

> **路线图引用**: [`roadmap.md`](../roadmap.md) §P0-B / §P0-C
> **状态**: 已实现（2026-06-03）

---

## P0-B: Planner agent 化（对齐 Claude Code Dynamic Workflows）

### 背景

原方案用 `TaskType::from_description()` 关键字分类 + 6 个固定 `generate_*_script()` 模板，留有 5 处 `-- TODO: Populate items` 占位符，且从未接入 CLI（`bail!("NL planning not yet implemented")`）。

Claude Code Dynamic Workflows 的做法是：**由 Claude 动态生成编排脚本**（"compile once" 范式 —— 模型当编译器，NL → DSL 脚本，runtime 确定性执行），而**脚本本身不碰文件系统**，文件枚举/读写交给运行时 agent。Maestro 的 Lua 沙箱（[`runtime/sandbox.rs`](../../src/runtime/sandbox.rs) 已禁 `io`/`os`/`require`）天然符合这一约束。

> ⚠️ 早期"文件系统扫描 `scan_workdir_files()`"方案**已废弃**：它在生成脚本时把文件列表烘焙进 Lua，与 DW"脚本不碰 FS、agent 负责枚举"的设计相反。

### 方案：LLM agent 生成脚本

[`planner.rs`](../../src/planner.rs) 重写为：

```rust
pub async fn plan_workflow(
    task: &str,
    backend: Arc<dyn AgentBackend>,
    cfg: &PlannerConfig,
) -> Result<PlannedWorkflow, PlannerError>
```

流程：

1. **构造 prompt** = `LUA_DSL_REFERENCE`（编排 DSL 规范：10 个原语签名 + 编排准则 + 示例）+ 用户任务。
2. **调 backend**（复用 run 的同一个 backend）跑一个一次性规划 agent，读 `AgentResult.output`。
3. **提取脚本** `extract_script()`：把 output 强制成文本，抓第一个 ` ```lua ` 围栏块（无围栏退回整段 trim）。
4. **校验** `validate_generated()`：`runtime::validate_script()`（Lua 语法）+ 断言含 `report(`。
5. **重试**：校验失败把错误回灌进 prompt，最多 `max_retries`（默认 3）次；耗尽返回 `PlannerError::ExhaustedRetries`。

DSL 规范中的关键编排准则：脚本只编排、禁碰 FS/shell；默认 `pipeline`，需要全部结果才 `parallel`，验证类用 `converge`；fan-out 控制在 ~16；必须以 `report(...)` 结束；只输出一个 ```lua 块。

### CLI 接入

| 文件 | 改动 |
|------|------|
| `planner.rs` | 删除 `TaskType`/`from_description`/`analyze_task`/`generate_*_script`/`escape_lua_string`；新增 `LUA_DSL_REFERENCE` + `plan_workflow` + `extract_script`/`validate_generated`/重试 |
| `cli.rs` | `RunArgs` 加 `script: Option<String>`；fresh-run 解析新增 script 分支；`init_run` 用真实任务名 |
| `main.rs` | `run_workflow` NL 分支 → `plan_workflow` → 审批 → 经 `RunArgs.script` 传给 `cli::run`；删除 `bail!` |
| `planner.rs` 测试 | `test_plan_extracts_and_validates_script` / `test_plan_retries_on_invalid_then_fails` / `test_plan_rejects_missing_report` / `test_extract_*`（MockBackend 喂 canned 脚本，纯离线） |

### 已知限制

- 离线 plan 依赖真实 LLM backend；mock backend 返回 Null → planner 报 `ExhaustedRetries`（预期）。
- `AgentResult.output` 文本形状取决于 backend；真实 ACP backend（P0-A）落地后需校准 `extract_script` 的字段提取。

---

## P0-C: Resume —— 已由 workflow.lua + journal 满足

原计划给 `RunCheckpoint` 加 `workflow_path` 字段。**实际无必要**：

- [`cli.rs`](../../src/cli.rs) 的 `--resume` 已从 `run_dir/workflow.lua` 读脚本，**不依赖 `--workflow`**。
- journal 已实现缓存式 resume（[`sandbox.rs`](../../src/runtime/sandbox.rs) 中 `agent()`/`parallel()` 命中 `get_cached` 即跳过已完成 agent）。

这正是 Claude DW 的 resume 模型：**缓存已完成 agent 的结果**，而非确定性重新生成 plan。因此 `workflow_path` 字段计划取消，仅保留"`init_run` 写入真实任务名"这一项收尾（让 `list`/`status` 可辨识）。
