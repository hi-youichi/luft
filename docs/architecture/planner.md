# planner 模块架构

> **自然语言 → Lua 编排脚本。** 不用关键词分类填模板，而是让一个 LLM **agent** 把任务"编译"成一段 Lua 编排脚本；运行时再确定性地执行它。

源码：[`src/planner.rs`](../../src/planner.rs)

---

## 1. 职责与边界

`planner` 把用户的一句自然语言（`maestro run "<NL>"`）转成一段可执行的 Lua 脚本。它的设计哲学对标 Claude Code Dynamic Workflows 的"compile once"模型：

- **模型即编译器**：NL（源语言）→ Lua DSL（目标语言）。
- **运行时即解释器**：生成的脚本交给 [runtime](./runtime.md) 确定性执行。

```
   "审计这个仓库的安全问题"
            │
   plan_workflow(task, backend, cfg)
            │  build_prompt(DSL 规范 + task [+ 上次错误])
            ▼
   backend.run(planner_task)  ◄── 一个 LLM agent 写脚本
            │
   extract_script(output)  →  validate_generated(语法 + 含 report())
            │  失败则把错误喂回，重试 ≤ max_retries
            ▼
   PlannedWorkflow { script }  ──►  cli::run 执行
```

**边界**：planner **只产出脚本字符串**，不执行它。生成的脚本**自身从不碰文件系统/shell**（Lua 沙箱禁止）——所有真实工作都在脚本 `spawn` 的 `agent()` prompt 里。

---

## 2. 核心流程：带自纠错的生成-校验回环

`plan_workflow()` 是唯一入口：

```
last_error = ""
for attempt in 0..max_retries:
    prompt = build_prompt(task, 若 attempt>0 则附 last_error)
    output = run_planner_agent(backend, prompt, model)      # 一次性 RunContext，事件丢弃
    script = extract_script(output)                          # 取文本 → 抽 ```lua 块
        若取不到 → last_error="no lua block"; continue
    match validate_generated(script):                        # 语法 + 必含 report(
        Ok  → return PlannedWorkflow { script }
        Err → last_error = e                                 # 把校验错误喂给下一轮
return Err(ExhaustedRetries { attempts, last_error })
```

**自纠错**是关键设计：校验失败时，错误信息会拼进下一轮 prompt（"你上次的尝试被拒绝：…修正并重新输出"），让模型自己改对。

---

## 3. 关键组件

| 组件 | 职责 |
|------|------|
| `PlannerConfig` | `planner_model`（None=后端默认）、`max_retries`（默认 3） |
| `PlannedWorkflow` | 产出：已校验、已去围栏的 `script` |
| `PlannerError` | `Backend`（后端调用失败）/ `ExhaustedRetries`（耗尽重试） |
| `run_planner_agent` | 构造一次性 `RunContext`（事件丢弃）+ `AgentTask`，调 `backend.run` 取 `output` |
| `extract_script` | `output_to_text` + `extract_lua_block` |
| `output_to_text` | 从后端结构化输出里取文本：依次试 `script`/`message`/`text`/`content`/`output` 键，否则 JSON 字符串化 |
| `find_fenced_block` | 手写围栏扫描器（无 regex 依赖）：取第一个 ` ```lua ` 或无标注的围栏块，跳过其他语言块 |
| `validate_generated` | `runtime::validate_script`（Lua 语法）+ 断言脚本含 `report(` |
| `LUA_DSL_REFERENCE` | 交给模型的 DSL 规范常量（"目标语言"定义） |

---

## 4. LUA_DSL_REFERENCE：交给模型的"目标语言"

这个常量是 planner 的灵魂——它告诉模型可用的原语、执行模型与规则。**必须与 [runtime](./runtime.md) 的 `sandbox.rs` 注册的原语保持同步**。它声明了：

- **执行模型**：脚本是编排器，持有循环/分支/中间结果；只有 `report()` 的值返回用户；脚本在沙箱中、不能碰 io/os/文件/shell。
- **原语签名**：`agent`/`parallel`/`pipeline`/`converge`/`phase`/`log`/`budget`/`workflow`/`report`/`json`。
- **规则**：必须以 `report(<table>)` 结尾；不碰文件系统；fan-out 控制在 ~16 并发内；默认优先 `pipeline()`；用 `phase()`/`log()` 让进度可见；只输出单个 ` ```lua ` 块。
- 一个完整的"枚举文件 → 并行审计 → report"示例。

---

## 5. 设计决策与权衡

- **agent 写脚本，而非模板填空**：能处理开放式任务、表达任意控制流；代价是依赖模型质量、需要校验回环兜底。
- **校验只查"语法 + 必含 report()"**：轻量、快速、无需执行；不做语义/安全静态分析（沙箱在运行时兜底安全）。
- **手写围栏扫描器**：避免引入 regex 依赖，足够应付 ` ```lua ` 提取与多块跳过。
- **DSL 规范内嵌为常量**：单一事实源，但要求人工与 runtime 原语同步（漂移风险）。
- **planner 用与执行相同的 backend**：复用同一 LLM 通道，无需独立配置；planner agent 与工作流 agent 走一致的能力。

---

## 6. 当前状态与局限（v0.1）

- Planner 已通过真实后端验证（opencode），可自动生成多阶段 Lua 工作流（discovery → parallel analysis → synthesis 等），README 标注为 ✅。生成质量取决于后端模型。
- DSL 规范与 runtime 原语**靠人工同步**，没有自动一致性检查。
- 校验不验证脚本的运行时正确性（如 fan-out 是否真的有界、agent prompt 是否合理）。
- planner agent 的事件被丢弃（`run_planner_agent` 用一次性 channel），规划过程对 TUI/headless 不可见。

---

## 7. 调用位置

planner 由 [`main.rs`](../../src/main.rs) 的 `run_workflow` 在 **NL 模式**下调用：生成脚本 → （除非 `--approve`）打印脚本请用户确认 → 把脚本透传给 `cli::run` 执行。见 [cli.md](./cli.md)。

---

## 8. 相关文档

- 总览：[../architecture.md](../architecture.md)
- 下游：[runtime.md](./runtime.md)（执行生成脚本的原语与沙箱）、[cli.md](./cli.md)（NL → 规划 → 确认 → 执行的串接）
- 依赖：[core.md](./core.md)（`AgentBackend`、`AgentTask`、`RunContext`）
- 旧版设计稿：[../archive/planner.md](../archive/planner.md)、[../design/p0-planner-resume.md](../design/p0-planner-resume.md)
