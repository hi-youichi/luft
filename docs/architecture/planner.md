# planner 模块架构

> **状态**: 待完善 — 骨架文档，需补充详细内容。

> 自然语言 → Lua 脚本（agent 写脚本 + 校验重试）。planner 是 Luft 的"编译器"层：以用户自然语言任务为输入，经 LLM agent 生成一段 Lua 编排脚本，再交给 runtime 确定性执行。

源码：[`src/planner.rs`](../../src/planner.rs) ｜ 设计稿：[`docs/design/p0-planner-resume.md`](../../docs/design/p0-planner-resume.md)

---

## 1. 职责与边界

planner 把用户的自然语言描述编译成一段遵循 Lua DSL 规范的编排脚本。它不执行脚本（那是 runtime 的事），不碰调度/持久化/后端——只做 NL → Lua 的一次性翻译。

```
   main.rs                              planner.rs
   run_workflow                          plan_workflow(task, backend, cfg)
     │                                      │
     ├── NL task ──────────────────────────►│
     │                                      ├── build_prompt → DSL 参考 + 用户任务
     │                                      ├── run_planner_agent → 调 LLM backend
     │                                      ├── extract_script → 取 ```lua 围栏块
     │                                      ├── validate_generated → 语法 + 含 report()
     │                                      └── 重试循环：失败回灌，最多 max_retries 次
     │◄── PlannedWorkflow { script } ───────┤
     ▼
   cli::run → Runtime::execute(script)
```

**边界**：planner 只依赖 `core` 的 `AgentBackend` trait（调 LLM 生成脚本），输出一个纯文本 Lua 脚本字符串给 `cli`。不接触 runtime、scheduler、journal。

---

## 2. 核心结构

| 类型/函数 | 职责 |
|-----------|------|
| `PlannerConfig` | 配置：`planner_model`（可选覆盖模型）、`max_retries`（默认 3） |
| `PlannedWorkflow` | 产物：校验通过的 Lua 脚本字符串 |
| `PlannerError` | 错误：`Backend(String)` / `ExhaustedRetries { attempts, last_error }` |
| `plan_workflow()` | 主入口：构造 prompt → 调 agent → 提取 → 校验 → 重试循环 |
| `run_planner_agent()` | 封装一次 backend.run，丢弃事件，返回原始 JSON output |
| `extract_script()` | 从 agent output 中提取 Lua 脚本（支持多种 output 形状） |
| `extract_lua_block()` | 找第一个 ` ```lua ` 围栏块；无围栏则启发式剥离散文 |
| `validate_generated()` | Lua 语法校验 + 必须包含 `report(...)` 调用 |
| `build_prompt()` | 拼接 DSL 参考 + 任务 + 可选修复错误回灌 |

---

## 3. 工作流程

1. **构造 prompt**：`LUA_DSL_REFERENCE`（编排 DSL 规范：10 个原语签名 + 编排准则 + 示例）+ 用户任务文本。首次失败后拼接重试提示。
2. **调 LLM backend**：用 backend 跑一次性 agent，读 `AgentResult.output`。事件 channel 立即丢弃。
3. **提取脚本**：`output_to_text` 兜底收束成文本 → `find_fenced_block` 找第一个 ` ```lua ` 围栏块 → 无围栏时尝试整段当 Lua 或启发式剥离。
4. **校验**：`validate_script`（调用 mlua `chunk.compile` 检查语法）+ 断言含 `report(` 调用。
5. **重试**：校验失败时把错误消息回灌进 prompt，最多 `max_retries` 次；耗尽返回 `ExhaustedRetries`。

---

## 4. 关键设计决策

- **模型当编译器**：严格遵循 DW "compile once" 范式——模型生成脚本，runtime 确定性执行。脚本不碰 FS/shell（沙箱禁用 `io`/`os`）。
- **重试回灌**：不简单重试，而是把语法错误/缺失 `report()` 等信息告诉模型让其自行修正。
- **脚本提取策略**：优先取 ` ```lua ` 围栏块，兼容无围栏裸文本，最后 fallback 启发式剥离。适应不同 backend 的 output 形状差异。
- **PlannedWorkflow 轻量**：只包含脚本字符串，不携带中间产物。所有状态由 cli 管理。

---

## 5. 当前状态与局限

| 项目 | 状态 |
|------|------|
| plan_workflow 核心流程 | ✅ 实现 |
| extract_script / validate_generated | ✅ 实现 |
| 重试循环 + 错误回灌 | ✅ 实现 |
| CLI 集成 | ✅ main.rs run_workflow → plan_workflow → 审批 → cli::run |
| 单元测试 | ✅ 4 个测试覆盖提取、校验、重试耗尽、跨语言围栏 |
| LUA_DSL_REFERENCE 的维护同步 | ⚠️ 需与 runtime sandbox 注册的原语保持手动一致 |
| 真实 ACP backend 的 output 形状校准 | ⚠️ 当前 extract_script 的 output_to_text 基于启发式，真实 ACP 后端落地后需验证 |
| 离线 plan 依赖真实 LLM | ⚠️ mock backend 返回 Null → 预期报 ExhaustedRetries（纯离线不可用） |
| 脚本缓存/plan 复用 | ❌ 每次 fresh run 都重新 plan，无缓存机制 |

---

## 6. 另见

- [`docs/design/p0-planner-resume.md`](../../docs/design/p0-planner-resume.md) — planner agent 化 & resume 的设计决策与历史
- [`src/planner.rs`](../../src/planner.rs) — 全部实现 + 测试
- [`docs/sdk-reference.md`](../../docs/sdk-reference.md) — Lua DSL 原语的运行时签名
- [`docs/architecture/runtime.md`](./runtime.md) — Lua 沙箱与 execute 流程
- [`docs/architecture/cli.md`](./cli.md) — run 生命周期中 plan 的调用位置
