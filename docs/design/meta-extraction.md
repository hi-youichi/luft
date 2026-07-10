# 方案 B：Meta 提取 + 持久化

> **状态**：方案设计（待评审）
> **最后更新**：2025-08-19
> **目标**：让 Lua 脚本携带声明式 `meta`（phase 结构），planner 提取并持久化到 checkpoint，`luft phases` 直接从 checkpoint 读完整 phase 结构——不再依赖 events 重建。
> **参考**：[Claude Code Dynamic Workflows](https://code.claude.com/docs/en/workflows) · [lua-workflow-spec.md](../dev/lua-workflow-spec.md) §1.3

---

## 0. 问题陈述

### 当前状态（三层断裂）

| 层 | 设计 | 实际 |
|---|---|---|
| **Spec** (`lua-workflow-spec.md`) | `meta = { phases = {...} }` + `function main()` | ✅ 已定义 |
| **Planner** (`planner.rs`) | 应生成 meta + main() | ❌ 生成扁平脚本，无 meta，无 main() |
| **Runtime** (`sandbox.rs:93`) | 应 exec 顶层 → 调用 main() | ❌ `lua.load(script).exec()` 整体执行 |

结果：`phases` 命令拿不到 phase 结构（总数、label、预计 agent 数），只能从 `events.jsonl` 的事后事件重建。

### 方案 B 做什么

```
planner 生成脚本（带 meta + main）
    ↓
extract_meta(script) → PlanMeta      ← 新增
    ↓
PlannedWorkflow { script, meta }     ← 扩展
    ↓
RunSpec { script, meta }             ← 扩展
    ↓
RunCheckpoint { ..., workflow_meta } ← 扩展
    ↓
luft phases <run_dir>             ← 直接读 checkpoint
```

---

## 1. 脚本格式变更

### 1.1 新格式（planner 生成 + 手写工作流）

```lua
-- ① meta 声明（必须在前，纯数据）
meta = {
    phases = {
        { label = "discover",  detail = "扫描代码库",            agents = 3 },
        { label = "analyze",   detail = "分析每个模块的使用",     agents = 5 },
        { label = "report",    detail = "综合输出报告",           agents = 1 },
    },
    reasoning = "先发现再分析，最后综合"
}

-- ② 编排逻辑（可选包在 main() 里）
function main()
    phase("discover", 3)
    local r = agent({ prompt = "scan codebase" })
    -- ...
    report({ summary = "done" })
end
```

### 1.2 向后兼容（关键）

**Runtime 执行策略**（`sandbox.rs:90` `execute()`）改为：

```rust
pub fn execute(&self, script: &str) -> Result<serde_json::Value, ScriptError> {
    self.lua.load(script).exec()?;  // ① 执行顶层（meta 赋值 + function 定义）

    // ② 如果存在 main()，调用它；否则顶层已经执行完毕（旧格式兼容）
    let main_fn: Option<mlua::Function> = self.lua.globals().get("main").ok();
    if let Some(f) = main_fn {
        f.call(())?;
    }

    // ③ 读 report_sink
    let guard = self.report_sink.lock().unwrap();
    Ok(guard.clone().unwrap_or(serde_json::Value::Null))
}
```

- **新格式**：顶层只做赋值 + 定义 `main()`，`execute()` 先 exec 顶层再 call `main()`
- **旧格式**（现有 examples + 已落盘的 run）：没有 `main()` → `get("main")` 返 Nil → 顶层已执行完毕 → 直接走 report_sink
- **零破坏**：现有脚本不需要改动

### 1.3 meta 字段定义（对齐 spec §1.3 + Claude Code）

```lua
meta = {
    phases = {
        {
            label     = "discover",       -- 必填，string，须与 phase() 调用一致
            detail    = "扫描代码库",      -- 必填，string，一行描述
            agents    = 3,                -- 可选，integer，预计 agent 数（默认 0）
            depends_on = { 1 },           -- 可选，array<int>，依赖的 phase 序号（1-based）
        },
        -- ...
    },
    reasoning = "..."                     -- 可选，string，规划理由
}
```

与 spec §1.3 的唯一差异：`depends_on` 从 0-based 改为 **1-based**（与 `phase()` 返回的 phase_id 一致，避免 off-by-one）。旧 spec 的 0-based 没有实际使用者。

---

## 2. Meta 提取

### 2.1 提取策略：Stubbed VM

**核心问题**：不能直接 `lua.load(script).exec()` 来提取 meta，因为现有脚本的顶层可能调用 `agent()` 等 SDK 函数——在一个干净的 VM 里这些不存在，会报错。

**解决方案**：创建一个临时 Lua VM，注册所有 SDK 函数为 **no-op stub**，然后 exec 脚本，读 `meta` 全局变量。

```rust
// src/planner/meta.rs（新增文件）

/// 从脚本中提取 meta 声明。
///
/// 策略：在 stubbed VM 中执行脚本，所有 SDK 函数注册为 no-op，
/// 顶层赋值（meta = {...}）和函数定义（function main() ... end）
/// 是安全的——stub 拦截了一切有副作用的调用。
pub fn extract_meta(script: &str) -> Result<PlanMeta, MetaError> {
    let lua = Lua::new();

    // 注册 no-op stubs：phase/log/budget/agent/parallel/pipeline/report/json/workflow
    register_stubs(&lua)?;

    // 执行脚本（安全：所有副作用被 stub 拦截）
    lua.load(script).exec()?;

    // 读 meta 全局变量
    let meta_table: Option<mlua::Table> = lua.globals().get("meta").ok();
    let Some(meta_table) = meta_table else {
        return Ok(PlanMeta::default()); // meta 缺失 → 空 meta（不报错）
    };

    // 转换为 PlanMeta
    parse_plan_meta(&lua, &meta_table)
}

fn register_stubs(lua: &Lua) -> mlua::Result<()> {
    let globals = lua.globals();
    // phase(name, planned?) -> 1
    globals.set("phase", lua.create_function(|_, _: (String, Option<f64>)| Ok(1i64))?)?;
    // agent(opts) -> {ok=false, output=nil, status="stub", tokens=0, findings={}}
    globals.set("agent", lua.create_function(|_, _: mlua::Value| {
        Ok(mlua::Value::Nil) // 返回 Nil，脚本逻辑会走 ok=false 分支
    })?)?;
    // parallel(items, fn) -> {}
    globals.set("parallel", lua.create_function(|_, _: (mlua::Value, mlua::Value)| {
        Ok(vec![mlua::Value::Nil; 0]) // 空数组
    })?)?;
    // report(v) -> nil
    globals.set("report", lua.create_function(|_, _: mlua::Value| Ok(()))?)?;
    // log/budget/workflow -> no-op
    for name in ["log", "budget", "workflow"] {
        globals.set(name, lua.create_function(|_, _: ()| Ok(()))?)?;
    }
    // json
    let json = lua.create_table()?;
    json.set("encode", lua.create_function(|_, _: mlua::Value| Ok("null".to_string()))?)?;
    json.set("decode", lua.create_function(|_, _: String| Ok(mlua::Value::Nil))?)?;
    globals.set("json", json)?;
    // args + ctx（脚本可能读 args.xxx）
    globals.set("args", lua.create_table()?)?;
    let ctx = lua.create_table()?;
    ctx.set("run_id", "00000000-0000-0000-0000-0000000000")?;
    globals.set("ctx", ctx)?;
    Ok(())
}
```

### 2.2 为什么不用其他方案

| 方案 | 问题 |
|---|---|
| **正则提取** `meta = \{...\}` | Lua table 嵌套大括号 + 注释 + 字符串里的 `}` → 正则不可靠 |
| **AST 解析** | 需引入 Lua parser crate（`full-moon` 等），依赖重 |
| **Planner 结构化输出**（meta 和 script 分开） | 改 planner prompt 太大，且手写工作流没法用 |
| **spec 的裸 exec**（`lua.load(script).exec()` 无 stub） | 脚本顶层调 `agent()` → VM 里没有 → 报错 |

Stubbed VM 是**最小侵入、最健壮**的方案：利用 mlua 自身解析 + 执行，只需要几十行 stub 代码。

### 2.3 数据结构

```rust
// src/planner/meta.rs

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlanMeta {
    pub phases: Vec<MetaPhase>,
    #[serde(default)]
    pub reasoning: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaPhase {
    pub label: String,
    pub detail: String,
    #[serde(default)]
    pub agents: usize,
    #[serde(default)]
    pub depends_on: Vec<u32>,
}
```

### 2.4 验证（提取后）

```rust
/// 校验 meta 与脚本的一致性。
fn validate_meta(meta: &PlanMeta, script: &str) -> Result<(), MetaError> {
    // 1. phase() 调用的 label 应该在 meta.phases 里
    let phase_labels: HashSet<&str> = meta.phases.iter()
        .map(|p| p.label.as_str()).collect();

    for line in script.lines() {
        if let Some(label) = extract_phase_call(line) {
            if !phase_labels.contains(label.as_str()) {
                // 软警告：phase() 调用了 meta 里没有的 label
                tracing::warn!(label, "phase() call not found in meta.phases");
            }
        }
    }

    // 2. depends_on 索引越界检查
    for (i, phase) in meta.phases.iter().enumerate() {
        for &dep in &phase.depends_on {
            if dep == 0 || dep as usize > meta.phases.len() {
                return Err(MetaError::InvalidDependency { phase: i + 1, dep });
            }
        }
    }

    Ok(())
}

/// 从 `phase("xxx")` 行提取 label（简单正则，不做完整 parse）。
fn extract_phase_call(line: &str) -> Option<String> {
    let line = line.trim();
    if !line.starts_with("phase(") { return None; }
    // 取第一个引号内的内容
    let after = line["phase(".len()..].trim_start();
    // ... 简单解析 ...
}
```

校验是**软约束**——meta.phases 和 phase() 调用不一致时只 `tracing::warn!`，不阻止运行。原因：Lua 工作流可以是动态的（循环里调 phase()），静态分析无法完全覆盖。

---

## 3. Planner 改动

### 3.1 `PlannedWorkflow` 扩展

```rust
// src/planner.rs

#[derive(Debug, Clone)]
pub struct PlannedWorkflow {
    pub script: String,
    pub meta: PlanMeta,          // ← 新增
}
```

### 3.2 `plan_workflow()` 改动

```rust
pub async fn plan_workflow(...) -> Result<PlannedWorkflow, PlannerError> {
    for attempt in 0..attempts {
        // ... 生成 script（同现有） ...

        match validate_generated(&script) {
            Ok(()) => {
                // ← 新增：提取 meta
                let meta = extract_meta(&script).unwrap_or_default();
                validate_meta(&meta, &script).ok(); // 软校验
                return Ok(PlannedWorkflow { script, meta });
            }
            Err(e) => { /* ... 同现有 ... */ }
        }
    }
    // ... 同现有 ...
}
```

### 3.3 `LUA_DSL_REFERENCE` 改动

在 prompt 开头加入 meta 要求：

```
# Script structure

Every script MUST begin with a `meta` declaration — a pure data table describing
the workflow's phase structure:

```lua
meta = {
    phases = {
        { label = "discover", detail = "Scan codebase for definitions", agents = 3 },
        { label = "analyze",  detail = "Analyze usage patterns",        agents = 5 },
        { label = "report",   detail = "Synthesize final report",       agents = 1 },
    },
    reasoning = "Discover first, then analyze, then synthesize"
}
```

Then wrap the orchestration logic in `function main() ... end`:

```lua
function main()
    phase("discover", 3)
    -- agent calls here
    report({ summary = "done" })
end
```

Fields:
- `phases[i].label` (string, required) — MUST match the string passed to phase()
- `phases[i].detail` (string, required) — one-line description
- `phases[i].agents` (integer, optional) — planned agent count for this phase
- `phases[i].depends_on` (array of integers, optional) — 1-based phase indices
- `reasoning` (string, optional) — why this structure

Rules:
- `meta` MUST be a pure literal table (no function calls, no variable references)
- The script body MUST be inside `function main() ... end`
- Use the SAME label string in meta.phases[i].label and phase("label")
```

### 3.4 `validate_generated()` 改动

```rust
fn validate_generated(script: &str) -> Result<(), String> {
    validate_script(script).map_err(|e| format!("lua syntax error: {}", e))?;
    if !script.contains("report(") {
        return Err("script must call report(...)".to_string());
    }
    // ← 新增：如果有 meta=，检查是否包含 phases 字段
    if script.contains("meta") && !script.contains("phases") {
        return Err("meta declaration must contain phases field".to_string());
    }
    Ok(())
}
```

---

## 4. 持久化改动

### 4.1 `RunSpec` 扩展

```rust
// src/service/run.rs

pub struct RunSpec {
    pub run_id: RunId,
    pub run_dir_name: String,
    pub script: String,
    pub task_label: String,
    pub resuming: bool,
    pub extra_args: serde_json::Value,
    pub workflow_meta: PlanMeta,       // ← 新增
}
```

### 4.2 `resolve_fresh()` 改动

```rust
pub async fn resolve_fresh(source, backend) -> Result<RunSpec> {
    let script = resolve_script(source, backend).await?;
    let meta = match source {
        ScriptSource::Nl(_) => {
            // planner 已经提取了 meta
            // 需要 resolve_script 返回 (script, meta)
            todo!() // 见下方 §4.3
        }
        ScriptSource::Workflow(path) => {
            // 手写工作流：提取 meta（可能为空）
            let script_content = std::fs::read_to_string(path)?;
            extract_meta(&script_content).unwrap_or_default()
        }
        ScriptSource::Script(s) => {
            extract_meta(s).unwrap_or_default()
        }
    };
    // ...
}
```

### 4.3 `resolve_script()` 返回值变更

```rust
/// 返回 (script, meta)
pub async fn resolve_script(
    source: ScriptSource<'_>,
    backend: Arc<dyn AgentBackend>,
) -> Result<(String, PlanMeta)> {
    match source {
        ScriptSource::Nl(nl) => {
            let planned = plan_workflow(nl, backend, &cfg).await?;
            Ok((planned.script, planned.meta))
        }
        ScriptSource::Workflow(path) => {
            let script = std::fs::read_to_string(path)?;
            let meta = extract_meta(&script).unwrap_or_default();
            Ok((script, meta))
        }
        ScriptSource::Script(s) => {
            let meta = extract_meta(s).unwrap_or_default();
            Ok((s.to_string(), meta))
        }
    }
}
```

### 4.4 `RunCheckpoint` 扩展

```rust
// src/core/state.rs

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunCheckpoint {
    pub run_id: RunId,
    pub task: String,
    pub status: CheckpointStatus,
    pub current_phase: u32,
    pub completed_phases: Vec<PhaseSummary>,
    pub agent_results: HashMap<AgentId, AgentResultCache>,
    pub findings: Vec<Finding>,
    pub total_tokens: u64,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]                        // ← 向后兼容旧 checkpoint
    pub workflow_meta: Option<PlanMeta>,     // ← 新增
}
```

用 `Option<PlanMeta>` + `#[serde(default)]`：旧 checkpoint 反序列化时 `workflow_meta = None`，不影响 resume。

### 4.5 写入时机

`workflow_meta` 在 **run 初始化时写入 checkpoint**（`prepare()` 里），不在运行中更新——meta 是静态声明，不随执行变化。

```rust
// src/service/run.rs — prepare()
if !spec.resuming {
    journal.init_run(spec.run_id, &spec.task_label)?;
    std::fs::write(run_dir.join("workflow.lua"), &spec.script)?;
    // ← 新增：把 meta 写入初始 checkpoint
    let store = journal.store();
    let mut cp = store.read_checkpoint().unwrap_or_default();
    cp.workflow_meta = Some(spec.workflow_meta.clone());
    store.write_checkpoint(&cp)?;
}
```

---

## 5. `luft phases` 简化

### 5.1 数据来源变化

| 字段 | 方案 A（从 events 重建） | 方案 B（从 checkpoint 读） |
|---|---|---|
| phase 总数 | `events` 的 `PhaseStart` 去重 | `checkpoint.workflow_meta.phases.len()` |
| phase label | `events` 的 `PhaseStart.label` | `checkpoint.workflow_meta.phases[i].label` |
| phase detail | ❌ 没有 | `checkpoint.workflow_meta.phases[i].detail` |
| 预计 agent 数 | `events` 的 `PhaseStart.planned` | `checkpoint.workflow_meta.phases[i].agents` |
| 依赖关系 | ❌ 没有 | `checkpoint.workflow_meta.phases[i].depends_on` |
| 实际 ok/failed | `checkpoint.completed_phases` | `checkpoint.completed_phases`（同） |
| running agent | `events` 的 `AgentStart`（无配对 Done） | 同（events 仍需用于 running 状态） |

**结论**：meta 提供**静态结构**（phase 树），events 提供**动态状态**（哪些 agent 正在跑）。两者合并，不再需要从 events 重建结构。

### 5.2 `build_phases_view()` 简化

```rust
pub fn build_phases_view(checkpoint: &RunCheckpoint, events: &[AgentEvent]) -> PhasesView {
    // 1. 从 meta 拿 phase 结构（静态）
    let meta_phases = checkpoint.workflow_meta
        .map(|m| m.phases)
        .unwrap_or_default();

    let phases: Vec<PhaseRow> = meta_phases.iter().enumerate().map(|(i, mp)| {
        let phase_id = (i + 1) as u32;
        let completed = checkpoint.completed_phases.iter()
            .find(|p| p.phase_id == phase_id);
        let running_agents = extract_running_agents(events, phase_id);

        PhaseRow {
            phase_id,
            label: mp.label.clone(),
            detail: mp.detail.clone(),
            planned: mp.agents,
            status: phase_status(phase_id, checkpoint, events),
            ok: completed.map(|p| p.ok).unwrap_or(0),
            failed: completed.map(|p| p.failed).unwrap_or(0),
            elapsed_secs: phase_elapsed(events, phase_id),
            agents: build_agent_rows(checkpoint, events, phase_id),
        }
    }).collect();

    PhasesView { run: run_header(checkpoint, events), phases }
}
```

### 5.3 降级（meta 为空时）

如果 `workflow_meta` 是 `None`（旧 checkpoint 或手写脚本没有 meta），回退到方案 A 的 events 重建逻辑。两条路径共存：

```rust
let phases = if let Some(ref meta) = checkpoint.workflow_meta {
    build_from_meta(meta, checkpoint, events)      // 方案 B 路径
} else {
    rebuild_from_events(events, checkpoint)         // 方案 A 降级路径
};
```

---

## 6. Runtime `execute()` 改动

```rust
// src/runtime/sandbox.rs

pub fn execute(&self, script: &str) -> Result<serde_json::Value, ScriptError> {
    let start = std::time::Instant::now();

    // ① 执行顶层（meta 赋值 + function 定义）
    self.lua.load(script).exec()?;

    // ② 如果脚本定义了 main()，调用它
    if let Ok(main_fn) = self.lua.globals().get::<mlua::Function>("main") {
        main_fn.call(())?;
    }
    // ③ 否则：顶层已执行完毕（旧格式兼容），什么都不做

    let elapsed = start.elapsed();
    let guard = self.report_sink.lock().unwrap();
    Ok(guard.clone().unwrap_or(serde_json::Value::Null))
}
```

`validate_script()` 不需要改——它只检查语法，对新旧格式都有效。

---

## 7. 改动清单

| 文件 | 改动 | 大小 |
|---|---|---|
| `src/planner/meta.rs` | **新增**：`PlanMeta`, `MetaPhase`, `extract_meta()`, `register_stubs()`, `validate_meta()` | ~150 行 |
| `src/planner.rs` | `PlannedWorkflow` 加 `meta` 字段；`plan_workflow()` 提取 meta；`validate_generated()` 加 meta 检查；`LUA_DSL_REFERENCE` 加 meta+main 段落 | ~80 行改动 |
| `src/core/state.rs` | `RunCheckpoint` 加 `workflow_meta: Option<PlanMeta>`（`#[serde(default)]`） | ~5 行 |
| `src/service/run.rs` | `RunSpec` 加 `workflow_meta`；`resolve_script()` 返回 `(String, PlanMeta)`；`resolve_fresh()` 填充 meta；`prepare()` 写入 checkpoint | ~40 行改动 |
| `src/runtime/sandbox.rs` | `execute()` 加 main() 调用逻辑 | ~5 行 |
| `src/service/phases.rs` | **新增**（同方案 A）：`build_phases_view()`，优先从 meta 读，降级到 events | ~120 行 |
| `src/commands/phases.rs` | **新增**（同方案 A）：CLI 命令 | ~60 行 |
| `src/commands/mod.rs` | 加 `pub mod phases;` | 1 行 |
| `src/main.rs` | `Commands` 枚举加 `Phases` 分支 | ~10 行 |
| `src/planner/mod.rs` | 如果 planner 是模块目录，加 `pub mod meta;` | 1 行 |

**总计**：新增 ~330 行，改动 ~140 行。

---

## 8. 测试计划

### 8.1 `planner/meta.rs` 单测

| 用例 | 场景 | 断言 |
|---|---|---|
| `extract_meta_basic` | 脚本含 `meta = { phases = {{label="a", detail="b"}} }` | `PlanMeta.phases.len() == 1` |
| `extract_meta_with_main` | meta + `function main() agent({}) end` | meta 提取成功，agent() 被 stub |
| `extract_meta_no_meta` | 纯脚本无 meta | `PlanMeta::default()`（空） |
| `extract_meta_stubbed_agent` | 顶层直接调 `agent()`（旧格式） | 不报错，返回空 meta |
| `extract_meta_parallel_stub` | 脚本调 `parallel(items, fn)` | 返回空数组，不 panic |
| `validate_meta_label_mismatch` | meta.phases 有 "a"，脚本调 `phase("b")` | 软警告，不报错 |
| `validate_meta_dep_out_of_bounds` | `depends_on = {99}` | `Err(MetaError::InvalidDependency)` |

### 8.2 `runtime/sandbox.rs` 单测

| 用例 | 场景 | 断言 |
|---|---|---|
| `execute_with_main` | 脚本有 `function main() report({}) end` | report_sink 有值 |
| `execute_without_main` | 旧格式脚本 `report({})` 在顶层 | report_sink 有值（兼容） |
| `execute_meta_no_side_effect` | meta 顶层赋值不触发 agent | 执行成功，无 agent 事件 |

### 8.3 `service/run.rs` 单测

| 用例 | 场景 | 断言 |
|---|---|---|
| `resolve_fresh_extracts_meta` | NL → planner → 脚本带 meta | `spec.workflow_meta.phases` 非空 |
| `resolve_fresh_workflow_meta` | 手写 .lua 带 meta | `spec.workflow_meta.phases` 非空 |
| `prepare_writes_meta_to_checkpoint` | fresh run prepare | checkpoint.json 含 `workflow_meta` |

### 8.4 `service/phases.rs` 单测

| 用例 | 场景 | 断言 |
|---|---|---|
| `phases_from_meta` | checkpoint 有 meta | 树形输出含全部 meta phase |
| `phases_fallback_no_meta` | checkpoint 无 meta（旧 run） | 降级到 events 重建 |
| `phases_mixed` | 2 completed + 1 running + 1 pending | 正确状态标签 |

### 8.5 向后兼容测试

- 用 `.luft/runs/` 下现有的 run（无 meta 的 checkpoint）跑 `phases` → 走降级路径，不报错。
- 用现有 `examples/*.lua`（无 main()）跑 `run` → 正常执行。

---

## 9. 实施顺序

```
Step 1  src/planner/meta.rs       — PlanMeta + extract_meta + stubs + 单测
Step 2  src/planner.rs            — PlannedWorkflow 加 meta + plan_workflow 集成 + DSL_REFERENCE 更新
Step 3  src/runtime/sandbox.rs    — execute() 加 main() 调用 + 单测
Step 4  src/core/state.rs         — RunCheckpoint 加 workflow_meta
Step 5  src/service/run.rs        — RunSpec + resolve_script + prepare 写 meta
Step 6  cargo test                — 全量测试通过（含旧 checkpoint 兼容）
Step 7  src/service/phases.rs     — build_phases_view（meta 优先 + events 降级）
Step 8  src/commands/phases.rs    — CLI 渲染 + 单测
Step 9  src/main.rs + mod.rs      — 注册命令
Step 10 手动验证                   — cargo run -- run --backend mock → phases → 树形输出
```

每一步可独立编译 + 测试，不破坏现有功能。

---

## 10. 不在范围内

- **预执行审批 UI**：Claude Code 用 meta.phases 展示「将如何运行」给用户确认。Luft CLI 暂不做（未来 TUI 可加）。
- **meta.phases 严格匹配 phase() 调用**：Lua 可动态调 phase()，静态校验只做 warn。
- **depends_on 的执行序约束**：meta 声明依赖但不强制——执行序由脚本控制。
- **converge 的 phase_id 修复**：converge 当前被禁用，单独处理。
