# Luft 示例运行指南

本文档描述所有内置示例的运行步骤，从简单到复杂，每一步都带有事件日志落盘和结果断言。

## 前置条件

```bash
cargo build

# Mock backend（无需外部依赖）
# OpenCode backend（需安装 opencode >= 1.17.0）
opencode --version

# 断言脚本依赖 jq
jq --version
```

## CLI 参数速查

| 参数 | 说明 |
|---|---|
| `-w, --workflow <FILE>` | Lua 工作流文件路径 |
| `-b, --backend <ID>` | 后端：`mock` 或 `opencode` |
| `-c, --confirm` | 执行前展示脚本并等待确认 |
| `--headless` | JSONL 模式输出到 stdout |
| `-o, --output <FILE>` | 将 report 写入文件（Markdown 自动提取） |
| `--log <FILE>` | 事件日志落盘 |
| `--log-format <FORMAT>` | 日志格式：`pretty`（默认）或 `jsonl` |
| `--args <JSON>` | 传入 Lua `args` 全局变量 |

---

## 第 1 级：Mock Backend（秒级）

Mock 返回固定 `Value::Null` + 空 findings，验证 SDK 接线和事件流完整性。

### 1.1 hello.lua — 最简 agent 调用

```bash
cargo run -- run -w examples/hello.lua -b mock \
    --log .Luft/example_logs/hello.jsonl --log-format jsonl
```

**预期：** 退出码 0，report 中 `status == "ok"`。

**日志中会出现的事件：**

```jsonl
{"type":"run_started","run_id":"...","task":"examples/hello.lua","ts":"..."}
{"type":"agent_started","run_id":"...","phase_id":0,"agent_id":"...","prompt_preview":"Say hello in exactly 3 words","model":"mock"}
{"type":"agent_done","run_id":"...","agent_id":"...","status":"ok","tokens":{...},"elapsed_ms":10}
{"type":"report_emitted","run_id":"...","phase_id":0,"report":{...}}
{"type":"run_done","run_id":"...","status":"Completed","total_tokens":{...},"report":{...}}
```

### 1.3 parallel-demo.lua — 并行处理

```bash
cargo run -- run -w examples/parallel-demo.lua -b mock \
    --log .Luft/example_logs/parallel.jsonl --log-format jsonl
```

**预期：** `total_files == 3`，每个 `results[i].status == "ok"`，`total_findings == 0`。

**日志中会出现的事件：**

```jsonl
{"type":"run_started",...}
{"type":"phase_started","phase_id":1,"label":"并行审查","planned":3}
{"type":"parallel_started","phase_id":1,"span_id":0,"count":3}       ← 并发启动 3 个 agent
{"type":"agent_started","agent_id":"...","prompt_preview":"审查这个文件: src/main.rs",...}
{"type":"agent_started","agent_id":"...","prompt_preview":"审查这个文件: src/lib.rs",...}
{"type":"agent_started","agent_id":"...","prompt_preview":"审查这个文件: src/cli.rs",...}
{"type":"agent_done","status":"ok",...}  × 3
{"type":"parallel_done","phase_id":1,"span_id":0,"ok":3,"failed":0,...}  ← span_id 与 started 配对
{"type":"report_emitted",...}
{"type":"run_done",...}
```

### 1.4 pipeline-demo.lua — 多阶段管道

```bash
cargo run -- run -w examples/pipeline-demo.lua -b mock \
    --log .Luft/example_logs/pipeline.jsonl --log-format jsonl
```

**预期：** `ok == 3`，`failed == 0`，`total_stages == 2`。

**日志中会出现的事件：**

```jsonl
{"type":"run_started",...}
{"type":"pipeline_started","total_stages":2,"items":3}                ← 3 个 item 流经 2 个阶段
{"type":"pipeline_stage_started","stage_index":0,"label":"research",...}
{"type":"agent_started",...}  × 3                                     ← 3 个 item 并发进入 research 阶段
{"type":"agent_done",...}  × 3
{"type":"pipeline_item_done","stage_index":0,"item_index":0,...}  × 3
{"type":"pipeline_stage_started","stage_index":1,"label":"summarize",...}
{"type":"agent_started",...}  × 3                                     ← 3 个 item 进入 summarize 阶段
{"type":"agent_done",...}  × 3
{"type":"pipeline_item_done","stage_index":1,...}  × 3
{"type":"pipeline_done","stages_completed":2,"total_ok":3,"total_failed":0}
{"type":"report_emitted",...}
{"type":"run_done",...}
```

### 1.5 converge-demo.lua — 对抗收敛（mock）

```bash
cargo run -- run -w examples/converge-demo.lua -b mock \
    --log .Luft/example_logs/converge-mock.jsonl --log-format jsonl
```

**预期：** `converged == true`，`rounds == 0`（mock 无 findings，立即终止）。

**日志中会出现的事件：**

```jsonl
{"type":"run_started",...}
{"type":"phase_started","phase_id":1,"label":"对抗性验证","planned":6}
{"type":"converge_started","span_id":0,"items":3,"max_rounds":3}      ← 3 个 claim，最多 3 轮
{"type":"converge_done","span_id":0,"rounds":0,"converged":true,...}  ← mock 无 findings → 0 轮终止
{"type":"report_emitted",...}
{"type":"run_done",...}
```

> Mock 的 converge 只验证了"空输入边界"，核心多轮对抗逻辑未覆盖。见 2.1 用 opencode 重跑。

---

## 第 2 级：OpenCode Backend（分钟级）

Agent 调用真实 LLM，验证数据流、多轮迭代和输出质量。

### 2.1 converge-demo.lua — 对抗收敛（opencode）

```bash
cargo run -- run -w examples/converge-demo.lua -b opencode \
    --log .Luft/example_logs/converge-opencode.jsonl --log-format jsonl
```

**预期：** `rounds >= 1`，`converged == true`。这是 mock 覆盖不到的核心路径：
- 多轮 produce → adversarial verify 循环
- 投票淘汰机制（`vote_threshold == 0.7`）
- 收敛判定或达到 `max_rounds == 3`

**日志中会出现的事件（多轮迭代）：**

```jsonl
{"type":"run_started",...}
{"type":"phase_started","label":"对抗性验证",...}
{"type":"converge_started","span_id":0,"items":3,"max_rounds":3}
{"type":"agent_started","prompt_preview":"produce findings for: API 端点 /users 需要 RBAC 鉴权",...}  × 3  ← 第 1 轮 producer
{"type":"agent_done","status":"ok",...}  × 3
{"type":"agent_started","prompt_preview":"adversary: refute or confirm...",...}  × N        ← 第 1 轮 adversarial verify
{"type":"agent_done",...}  × N
{"type":"agent_started",...}  × 3                                                              ← 第 2 轮 producer（如未收敛）
...
{"type":"converge_done","span_id":0,"rounds":2,"converged":true,"surviving":2,...}
{"type":"report_emitted",...}
{"type":"run_done",...}
```

### 2.2 schema-demo.lua — Schema 结构化输出

```bash
cargo run --bin Luft -- run -w examples/schema-demo.lua -b opencode \
    --log .Luft/example_logs/schema.jsonl --log-format jsonl
```

**预期：** `extracted.name` 存在（真实的 LLM 输出），`eval.approved >= 0`，`summary` 非空。

**演示要点：**
- 定义 JSON Schema（`PERSON_SCHEMA`、`FINDING_SCHEMA`、`REPORT_SCHEMA`）约束 agent 输出
- `agent()` 调用中传 `schema = MY_SCHEMA`，结果通过 `result.output.field_name` 按字段访问
- `parallel(items, mapperFn)` 中 mapper 返回值携带独立 schema
- 结构化数据跨 agent 传递：extract → parallel validate → summarize
- 使用 `safe_agent`（`pcall` 包装）使脚本在 mock 后下降级可用

**日志中会出现的事件：**

```jsonl
{"type":"run_started",...}
{"type":"agent_started","prompt_preview":"You are analyzing the Luft project contributors...",...}
{"type":"agent_done","status":"ok","output":{"name":"...","role":"...","languages":[...],"yoe":...}}
{"type":"parallel_started","count":3,...}
{"type":"agent_started",...}  × 3
{"type":"agent_done",...}  × 3
{"type":"parallel_done",...}
{"type":"agent_started","prompt_preview":"Generate a brief one-paragraph summary...",...}
{"type":"agent_done",...}
{"type":"report_emitted","report":{"extracted":{...},"eval":{...},"summary":"..."}}
{"type":"run_done",...}
```

> 也可用 mock 运行验证接线：`cargo run --bin Luft -- run -w examples/schema-demo.lua -b mock`（所有 agent 降级到 fallback 数据，验证错误处理路径）。

### 2.3 deep-research.lua — 多智能体深度研究

```bash
cargo run -- run -w examples/deep-research.lua -b opencode \
    -o deep-research.md \
    --log .Luft/example_logs/deep-research.jsonl --log-format jsonl
```

**工作流四阶段：** plan → research（并行）→ synthesize → verify

**预期：**
- `sub_research_ok == sub_research_total`
- `deep-research.md` 包含 `# Claude Code Dynamic Workflows` 标题 + 至少 3 个 `##` 二级标题
- 报告末尾有 `## Confidence & Caveats` 章节

**日志中会出现的事件（四阶段）：**

```jsonl
{"type":"run_started",...}
{"type":"budget_set","time_limit_ms":300000,"max_rounds":30}          ← budget() 设置资源上限

{"type":"phase_started","phase_id":1,"label":"plan","planned":1}
{"type":"agent_started","prompt_preview":"You are the lead researcher...","model":null,...}  ← 首席研究员拆解主题
{"type":"acp_raw","kind":"agent_message_chunk",...}  × 多条                                           ← LLM 流式输出
{"type":"agent_done","status":"ok","tokens":{...},...}

{"type":"phase_started","phase_id":2,"label":"research","planned":4}
{"type":"parallel_started","span_id":1,"count":4,...}                 ← 4 个子研究员并行调研
{"type":"agent_started",...}  × 4
{"type":"acp_raw",...}  × 多条
{"type":"agent_done",...}  × 4
{"type":"parallel_done","span_id":1,"ok":4,"failed":0,...}

{"type":"phase_started","phase_id":3,"label":"synthesize","planned":1}
{"type":"agent_started","prompt_preview":"You are a senior research analyst...",...}  ← 分析师综合报告
{"type":"agent_done",...}

{"type":"phase_started","phase_id":4,"label":"verify","planned":1}
{"type":"agent_started","prompt_preview":"You are a meticulous technical editor...",...}  ← 编辑润色
{"type":"agent_done",...}

{"type":"report_emitted","report":{"markdown":"# Claude Code Dynamic Workflows\n..."}}
{"type":"run_done","status":"Completed",...}
```

### 2.4 architecture-report.lua — 代码架构分析

```bash
cargo run -- run -w examples/architecture-report.lua -b opencode \
    -o architecture.md \
    --log .Luft/example_logs/architecture.jsonl --log-format jsonl
```

**工作流三阶段：** discovery → analysis（并行）→ synthesis

**预期：**
- `successful_analyses == modules_analyzed`
- `architecture.md` 为中文，包含 `## 1. 项目概述` / `## 2. 整体架构` / `## 3. 模块职责说明`
- 至少提及 core、runtime、adapters 三个模块，引用具体类型名

**日志中会出现的事件（三阶段）：**

```jsonl
{"type":"run_started",...}

{"type":"phase_started","phase_id":1,"label":"discovery","planned":1}
{"type":"agent_started","prompt_preview":"You are exploring the Luft...",...}  ← 探测 agent 枚举模块
{"type":"acp_raw",...}  × 多条                                                    ← LLM 读取文件 + 分析
{"type":"agent_done","status":"ok",...}

{"type":"phase_started","phase_id":2,"label":"analysis","planned":8}
{"type":"parallel_started","span_id":1,"count":8,...}                 ← 每个模块并行分析
{"type":"agent_started",...}  × 8
{"type":"agent_done",...}  × 8
{"type":"parallel_done","span_id":1,...}

{"type":"phase_started","phase_id":3,"label":"synthesis","planned":1}
{"type":"agent_started","prompt_preview":"You are a senior software architect...",...}  ← 架构师综合报告
{"type":"agent_done",...}

{"type":"report_emitted","report":{"markdown":"# Luft 架构概览报告\n..."}}
{"type":"run_done","status":"Completed",...}
```

---

## 事件日志参考

`--log` 落盘的 JSONL 文件中，每行是一个 `AgentEvent`，通过 `"type"` 字段区分。所有事件共享 `"run_id"` 字段用于关联。

### 生命周期事件（每次运行必有）

| 事件 | `"type"` 值 | 含义 | 关键字段 |
|---|---|---|---|
| RunStarted | `"run_started"` | 运行启动 | `task`: 任务描述，`ts`: 启动时间 |
| RunDone | `"run_done"` | 运行结束（终态事件） | `status`: `Completed`/`Failed`/`Cancelled`/`Partial`；`report`: 最终报告；`total_tokens`: 总 token 消耗 |
| PhaseStarted | `"phase_started"` | 阶段开始（`phase()` 调用） | `phase_id`: 阶段序号；`label`: 阶段名称；`planned`: 预计 agent 数 |
| PhaseDone | `"phase_done"` | 阶段结束 | `phase_id`；`ok`/`failed`: 成功/失败 agent 数 |

### Agent 事件

| 事件 | `"type"` 值 | 含义 | 关键字段 |
|---|---|---|---|
| AgentStarted | `"agent_started"` | 单个 agent 开始执行 | `agent_id`: agent 唯一标识；`prompt_preview`: prompt 前 72 字符；`model`: 使用的模型 |
| AgentDone | `"agent_done"` | agent 执行完成 | `status`: `Ok`/`Error`/`Timeout`；`tokens`: `{input, output, cache_read, cache_write}`；`elapsed_ms`: 耗时 |
| AgentProgress | `"agent_progress"` | agent 执行中间进度 | `delta`: `Message`（文本流式输出）/ `ToolCall`（工具调用）/ `FileEdit`（文件编辑）/ `Tokens`（token 更新） |

### SDK 原语事件

| 事件 | `"type"` 值 | 含义 | 关键字段 |
|---|---|---|---|
| BudgetSet | `"budget_set"` | 资源预算设置（`budget()` 调用） | `time_limit_ms`: 时间限制；`max_rounds`: 最大轮次 |
| ReportEmitted | `"report_emitted"` | 脚本产出报告（`report()` 调用） | `report`: 报告 JSON 值 |
| ParallelStarted | `"parallel_started"` | 并行分发开始（`parallel()` 调用） | `span_id`: 用于与 Done 配对；`count`: 并发 agent 数 |
| ParallelDone | `"parallel_done"` | 并行分发完成 | `span_id`（与 Started 相同）；`ok`/`failed`: 成功/失败数；`results`: 所有 agent 结果 |
| ConvergeStarted | `"converge_started"` | 对抗收敛开始（`converge()` 调用） | `span_id`；`items`: 输入 claim 数；`max_rounds`: 最大迭代轮次 |
| ConvergeDone | `"converge_done"` | 对抗收敛完成 | `span_id`；`rounds`: 实际迭代轮次；`converged`: 是否收敛；`surviving`: 存活的 finding 数 |
| WorkflowStarted | `"workflow_started"` | 子工作流启动（`workflow()` 调用） | `span_id`；`path`: 子脚本路径；`args`: 传入参数 |
| WorkflowDone | `"workflow_done"` | 子工作流完成 | `span_id`；`report`: 子工作流报告；`error`: 错误信息（成功时为 null） |

### Pipeline 事件

| 事件 | `"type"` 值 | 含义 | 关键字段 |
|---|---|---|---|
| PipelineStarted | `"pipeline_started"` | 管道执行开始（`pipeline()` 调用） | `total_stages`: 阶段数；`items`: item 数 |
| PipelineStageStarted | `"pipeline_stage_started"` | 管道某阶段开始 | `stage_index`: 阶段序号；`label`: 阶段名称；`agents_in_stage`: 涉及 agent 数 |
| PipelineItemDone | `"pipeline_item_done"` | 管道中某个 item 完成某阶段 | `stage_index`；`item_index`；`status`；`tokens`；`elapsed_ms` |
| PipelineDone | `"pipeline_done"` | 管道执行完成 | `stages_completed`；`total_ok`；`total_failed` |

### 其他事件

| 事件 | `"type"` 值 | 含义 | 关键字段 |
|---|---|---|---|
| Log | `"log"` | 脚本日志（`log()` 调用） | `level`: `trace`/`debug`/`info`/`warn`/`error`；`msg`: 日志消息 |
| AcpRaw | `"acp_raw"` | ACP 协议原始 session/update（仅 opencode backend） | `kind`: 更新类型（如 `agent_message_chunk`）；`raw`: 完整 ACP 消息 |

### span_id 配对规则

`parallel`、`converge`、`workflow` 三种 SDK 原语使用 `span_id` 关联 Started/Done 事件：
- 同一次调用的 `XxxStarted.span_id` 和 `XxxDone.span_id` **必须相同**
- 不同调用的 `span_id` **必须不同**
- 断言脚本会自动检查此配对关系

---

## 自动断言

一键运行全部示例并自动验证结果：

```bash
bash scripts/run_examples.sh           # 全部（mock + opencode）
bash scripts/run_examples.sh mock      # 仅 mock（秒级）
bash scripts/run_examples.sh opencode  # 仅 opencode（分钟级）
```

详见 `scripts/run_examples.sh`。

---

## 通用验证清单

| 检查项 | 方法 |
|---|---|
| 退出码 == 0 | `echo $?` |
| report 被调用 | stdout 包含 `=== Report ===` 或 JSONL 中有 `"type":"report_emitted"` |
| 事件流完整 | JSONL 中 `run_done.status == "Completed"` |
| span 配对正确 | 每个 `*_started` 都有对应 `*_done`，`span_id` 相同 |
| 无 panic | stderr 不含 `panicked` |
| token 统计合理 | opencode 案例中 `total_tokens > 0` |
| 报告非空 | `-o` 输出文件大小 > 0 |
