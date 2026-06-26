# Phase Span 分层编排方案

> 将大任务（如多模块重构）拆分为多个结构性单元，每个单元内部复用相似的工作流模板。
> 通过 `phase_begin`/`phase_end` 给 phase 加 span 语义（有生命周期、可嵌套），形成显式树形进度结构，支持 span 级 checkpoint/resume。
> 不引入新概念——只有 phase 一个概念，span 是它的变体。

---

## 1. 背景与问题

### 现状

当前 planner（`planner.rs:59`）一次性将 NL 任务编译为一个 Lua 脚本，脚本内的 `phase()` 是扁平的点标记（`control.rs:22`：`fetch_add` 递增 id + `PhaseStarted` 事件），没有生命周期、没有 parent 归属、没有嵌套。

### 痛点

1. **大任务进度不可读**：重构 10 个模块 × 3 步骤 = 30 个扁平 phase，人类无法快速定位"跑到哪个模块了"。
2. **无 span 级恢复语义**：journal 隐式重放虽然能恢复（cache key = prompt+model+phase_id），但 checkpoint 不记录"哪些结构单元已完成"，resume 后人类看不到结构化的进度断点。
3. **planner 无分拆引导**：`LUA_DSL_REFERENCE`（`planner.rs:285-474`）没有教导 LLM 如何对大任务做结构性分解。

### 目标

- 让 planner 能把大任务拆成多个结构单元（按模块/子系统），每个单元套相似模板（analyze → change → verify）。
- runtime 提供显式树形进度（span 有 begin/end），UI 能渲染嵌套结构。
- span 边界可落 checkpoint，resume 时能跳过已完成 span。

---

## 2. 设计决策

| 决策点 | 选择 | 理由 |
|--------|------|------|
| **层级机制** | phase 加 span 变体（`phase_begin`/`phase_end`），不新增独立概念 | 只有一个概念，降低 LLM 和用户的认知负担 |
| **API 风格** | `phase()` = 点标记（步骤级，不变）；`phase_begin()`/`phase_end()` = span（结构级，push/pop 栈） | 大单元用 span，步骤用点标记，职责清晰 |
| **树表达** | 单次 LLM 调用同时输出嵌套 span + Lua 脚本 | 树结构隐含在 phase_begin/phase_end 嵌套中 |
| **parent 归属** | `phase()` 自动读栈顶 span 作为 parent | 栈式：对 planner 自然，只需在单元开头 begin、结尾 end |
| **树深度** | 2 层默认（span + step），3 层大任务（group span + module span + step），runtime 不限 | 2 层覆盖模块清单已知的单层重构；3 层覆盖整 crate/monorepo 级 |
| **checkpoint 粒度** | `phase_end` 时落显式 checkpoint（记录已完成 span） | journal 隐式重放够快（毫秒级 cache 命中），但显式 checkpoint 提供观测性 + resume 跳过语义 |
| **边界谁定** | 混合：planner 静态写死 + 运行时 agent 动态枚举 | 重构场景需运行时扫代码库才知道有哪些模块 |

### 为什么不用 milestone

milestone 做的事和 phase span 完全重叠——push/pop 栈、parent 归属、checkpoint 边界。新增独立概念只增加认知负担（LLM 和用户都要多想"该用 milestone 还是 phase"），收益为零。统一为 phase 的 span 变体更简洁。

---

## 3. DSL 设计

### 3.1 原语总览

```lua
-- 点标记（步骤级，不变）
phase(name, planned?) -> phase_id

-- span（结构级，新增）
phase_begin(name, planned?) -> span_id   -- push 到 span 栈
phase_end(span_id?)                      -- pop（不传则弹栈顶）
```

**语义规则：**
- `phase()` 在 span 内调用时，自动挂栈顶 span 为 parent。
- `phase()` 在 span 外调用时，parent_span_id = None（与现有行为完全一致）。
- `phase_begin()` 消费同一个 `phase_counter`，因此 span_id 和 phase_id 共享递增空间，全局唯一。
- `phase_begin()` 可以嵌套（span 内再 span）。

### 3.2 基本 2 层示例

```lua
local m = phase_begin("重构 auth 模块")   -- push span，m = 1
  phase("分析")    -- PhaseStarted { phase_id=2, parent_span_id=1 }
  phase("重构")    -- PhaseStarted { phase_id=3, parent_span_id=1 }
  phase("验证")    -- PhaseStarted { phase_id=4, parent_span_id=1 }
phase_end(m)      -- pop span，落 checkpoint
```

### 3.3 嵌套 3 层示例

```lua
local g = phase_begin("重构 core/ 子系统")        -- depth=0, push
  local m1 = phase_begin("state.rs")              -- depth=1, push
    phase("分析")  phase("重构")  phase("验证")
  phase_end(m1)

  local m2 = phase_begin("scheduler/")            -- depth=1, push
    phase("分析")  phase("重构")  phase("验证")
  phase_end(m2)
phase_end(g)                                      -- depth=0, pop
```

### 3.4 动态枚举模式（重构场景核心）

```lua
-- 先让 agent 发现模块清单，再 fan-out
phase("枚举目标")
local discover = agent({
  prompt = "列出 src/ 下需要重构的模块，返回模块名和路径",
  schema = MODULES_SCHEMA
})
if not discover.ok then
  report({ error = "枚举失败: " .. discover.status })
  return
end

local results = {}
for _, mod in ipairs(discover.output.modules or {}) do
  local name = "重构 " .. mod.name

  -- resume 跳过已完成 span
  if completed_spans and completed_spans[name] then
    log("跳过已完成: " .. name)
    goto continue
  end

  local m = phase_begin(name)
    phase("分析")
    local a = agent({ prompt = "分析 " .. mod.path, schema = ANALYSIS_SCHEMA })

    phase("重构")
    local c = agent({ prompt = "重构 " .. mod.path, schema = CHANGES_SCHEMA })

    phase("验证")
    local v = agent({ prompt = "验证 " .. mod.path, schema = VERIFY_SCHEMA })

    table.insert(results, { module = mod.name, ok = v.ok })
  phase_end(m)

  ::continue::
end

report({ spans = #results, results = results })
```

### 3.5 并行 span 安全性

当前 `parallel()` 在 `parallel.rs` 中实现，每个 item 的 `mapFn` 闭包在同一个 Lua VM 中**顺序构造** opts table（不真正并行执行 Lua），实际 agent 调用由 scheduler 并行。因此 span 栈在 `mapFn` 执行期间是串行的——`phase_begin` → 构造 opts → `phase_end` 的模式在 `parallel()` 中是安全的。但 `phase_end` 必须在 `mapFn` return 前调用：

```lua
local results = parallel(targets, function(mod)
  local m = phase_begin("重构 " .. mod.name)
    phase("分析")
    -- 构造 opts table...
  phase_end(m)   -- 必须在 return 前 pop
  return { opts = ... }
end)
```

> **约束**：如果未来 `parallel()` 变成真并行 Lua 执行，需要 per-item span 栈副本。

---

## 4. Runtime 改动

### 4.1 SdkContext（`sdk/mod.rs`）

新增 span 栈：

```rust
pub struct SdkContext {
    // ... 现有字段 ...
    pub phase_counter: Arc<AtomicU32>,
    pub span_counter: Arc<AtomicU64>,
    /// 新增：phase span 栈。push/pop 由 phase_begin()/phase_end() 操作。
    /// phase() 读栈顶作为 parent_span_id。
    pub phase_span_stack: Arc<Mutex<Vec<PhaseSpan>>>,
}

#[derive(Debug, Clone)]
pub struct PhaseSpan {
    pub id: u32,
    pub name: String,
    pub parent_id: Option<u32>,
    pub depth: u32,
    pub started_at: std::time::Instant,
    pub planned: usize,
}
```

`SdkContext::new()` 初始化 `phase_span_stack: Arc::new(Mutex::new(Vec::new()))`。

### 4.2 control.rs — 新增 phase_begin / phase_end

在 `register_control_sdk` 中新增两个 Lua 全局函数：

```rust
// ---- phase_begin(name, planned?) -> id ---------------------------------
{
    let events = cx.events();
    let phase_counter = cx.phase_counter.clone();
    let phase_span_stack = cx.phase_span_stack.clone();
    let begin_fn = lua.create_function(move |_, (name, planned): (String, Option<f64>)| {
        let id = phase_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let planned = planned
            .map(|v| {
                if v.is_nan() || v < 0.0 { 0 }
                else if v > usize::MAX as f64 { usize::MAX }
                else { v as usize }
            })
            .unwrap_or(0);

        let (parent_id, depth) = {
            let stack = phase_span_stack.lock().unwrap();
            (stack.last().map(|s| s.id), stack.len() as u32)
        };

        let span = PhaseSpan {
            id, name: name.clone(), parent_id, depth,
            started_at: std::time::Instant::now(),
            planned,
        };

        tracing::info!(span_id = id, %name, parent_id = ?parent_id, depth, "phase span started");

        {
            let mut stack = phase_span_stack.lock().unwrap();
            stack.push(span);
        }

        let _ = events.send(AgentEvent::PhaseSpanStarted {
            run_id, span_id: id, name, parent_id, depth, planned,
        });
        Ok(id as i64)
    })?;
    globals.set("phase_begin", begin_fn)?;
}

// ---- phase_end(span_id?) -----------------------------------------------
{
    let events = cx.events();
    let phase_span_stack = cx.phase_span_stack.clone();
    let end_fn = lua.create_function(move |_, id: Option<i64>| {
        let mut stack = phase_span_stack.lock().unwrap();

        let span = match id {
            Some(target) => {
                let pos = stack.iter().rposition(|s| s.id as i64 == target)
                    .ok_or_else(|| mlua::Error::RuntimeError(
                        format!("phase_end: span id {} not found in stack", target)
                    ))?;
                stack.split_off(pos).remove(0)
            }
            None => {
                stack.pop().ok_or_else(|| mlua::Error::RuntimeError(
                    "phase_end: span stack is empty".to_string()
                ))?
            }
        };

        let elapsed_ms = span.started_at.elapsed().as_millis() as u64;
        tracing::info!(span_id = span.id, elapsed_ms, "phase span ended");

        let _ = events.send(AgentEvent::PhaseSpanDone {
            run_id, span_id: span.id, name: span.name,
            elapsed_ms, status: "completed".to_string(),
        });
        Ok(())
    })?;
    globals.set("phase_end", end_fn)?;
}
```

### 4.3 control.rs — phase() 改动

`phase()` 签名不变，增加读栈顶逻辑：

```rust
let phase_fn = lua.create_function(move |_, (label, planned): (String, Option<f64>)| {
    let phase_id = phase_counter.fetch_add(1, Ordering::Relaxed) + 1;
    let planned = /* 现有逻辑不变 */;

    // 新增：读栈顶作为 parent
    let parent_span_id = {
        let stack = phase_span_stack.lock().unwrap();
        stack.last().map(|s| s.id)
    };

    let _ = events.send(AgentEvent::PhaseStarted {
        run_id,
        phase_id,
        label,
        planned,
        parent_span_id,  // 新字段 (Option<u32>)
    });
    Ok(phase_id as i64)
})?;
```

### 4.4 事件类型（`contract/event.rs`）

```rust
pub enum AgentEvent {
    // ... 现有事件 ...

    /// phase span 开始。
    PhaseSpanStarted {
        run_id: RunId,
        span_id: u32,
        name: String,
        parent_id: Option<u32>,
        depth: u32,
        planned: usize,
    },
    /// phase span 结束。status: "completed" | "failed"。
    PhaseSpanDone {
        run_id: RunId,
        span_id: u32,
        name: String,
        elapsed_ms: u64,
        status: String,
    },
}
```

`PhaseStarted` 新增字段（加 `#[serde(default)]` 兼容旧日志）：

```rust
PhaseStarted {
    run_id: RunId,
    phase_id: PhaseId,
    label: String,
    planned: usize,
    #[serde(default)]
    parent_span_id: Option<u32>,  // 新增
},
```

### 4.5 Checkpoint 机制（`core/state.rs`）

#### RunCheckpoint 新增字段

```rust
pub struct RunCheckpoint {
    // ... 现有字段 ...
    /// 已完成的 phase span 列表（按完成顺序）。
    #[serde(default)]
    pub completed_spans: Vec<PhaseSpanSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseSpanSummary {
    pub id: u32,
    pub name: String,
    pub parent_id: Option<u32>,
    pub depth: u32,
    pub elapsed_ms: u64,
    pub completed_at: u64,
}
```

#### state.rs 的 update_from_event 扩展

```rust
fn update_from_event(&self, event: &AgentEvent) {
    // ... 现有匹配 ...
    match event {
        AgentEvent::PhaseSpanDone { span_id, name, elapsed_ms, .. } => {
            if let Some(ref mut cp) = *checkpoint_guard {
                cp.completed_spans.push(PhaseSpanSummary {
                    id: *span_id,
                    name: name.clone(),
                    parent_id: None,  // 从 PhaseSpanStarted 事件补全，或在此处省略
                    depth: 0,
                    elapsed_ms: *elapsed_ms,
                    completed_at: current_timestamp(),
                });
            }
        }
        _ => {}
    }
}
```

> **parent_id/depth 填充**：`PhaseSpanDone` 事件不携带 parent_id/depth（这些在 Started 时已知）。两种方案：
> - **方案 A**：checkpoint 同时监听 `PhaseSpanStarted`，在内存维护一个 `HashMap<span_id, PhaseSpan>`，Done 时合并写出。
> - **方案 B**（推荐）：`PhaseSpanDone` 事件直接带上 `parent_id` 和 `depth`（从 span 栈 pop 时已知），简化 checkpoint 逻辑。
>
> 选方案 B——在 `phase_end` 的 closure 中，span pop 出来时已经有完整信息，直接放进事件。

修正后的事件定义：

```rust
PhaseSpanDone {
    run_id: RunId,
    span_id: u32,
    name: String,
    parent_id: Option<u32>,
    depth: u32,
    elapsed_ms: u64,
    status: String,
},
```

### 4.6 改动文件清单

| 文件 | 改动类型 | 说明 |
|------|----------|------|
| `src/runtime/sdk/mod.rs` | 改 | SdkContext 加 `phase_span_stack` + `PhaseSpan` struct |
| `src/runtime/sdk/control.rs` | 改 | 新增 `phase_begin()` / `phase_end()` 注册；`phase()` 读栈顶填 `parent_span_id` |
| `src/runtime/sandbox.rs` | 改 | 更新模块注释（原语列表加 phase_begin/phase_end） |
| `src/core/contract/event.rs` | 改 | 新增 `PhaseSpanStarted` / `PhaseSpanDone`；`PhaseStarted` 加 `parent_span_id` |
| `src/core/state.rs` | 改 | `RunCheckpoint` 加 `completed_spans`；新增 `PhaseSpanSummary`；`update_from_event` 处理 span 事件 |

---

## 5. Planner 改动

### 5.1 LUA_DSL_REFERENCE 扩展（`planner.rs:285-474`）

在 `# Primitives` 段新增 phase_begin/phase_end 原语说明，在 `# Rules` 段新增使用准则，新增两个示例。

#### Primitives 段新增

```
- phase_begin(name, planned?) -> span_id
    BEGINS a structural phase span (push onto the span stack). Returns a unique id.
    Use to decompose large tasks into structural units (e.g., per-module refactoring).
    All phase() calls between phase_begin() and phase_end() are children of this span.
    Spans can nest.

- phase_end(span_id?)
    ENDS the current (or matching id) phase span (pop from stack).
    Emits a checkpoint — a completed span can be skipped on resume.
```

#### Rules 段新增

```
12. For large tasks (refactoring multiple modules, auditing multiple subsystems),
    decompose into phase spans. Each span wraps a similar internal workflow
    (e.g., analyze → change → verify). Put phase_begin()/phase_end() around
    each unit; use phase() for steps inside.
13. For unknown scopes, have an agent enumerate targets first, then loop with
    phase_begin() per target. Do NOT hardcode module names unless the task specifies them.
14. ALWAYS pair phase_begin() with phase_end(). Unpaired phase_begin() is a runtime error.
15. Spans can nest (2-3 levels). Use 2 levels by default (span + steps);
    use 3 levels (group span + module span + steps) for whole-crate/monorepo tasks.
16. When resuming (the `completed_spans` global is non-nil), skip spans whose name
    matches an entry. Use goto continue to skip.
```

#### 新增示例 1：静态分拆（2 层）

```
# Example: per-module refactoring (static decomposition)
```lua
local MODULES = { "auth", "db", "api" }
local results = {}

for _, mod in ipairs(MODULES) do
  local name = "refactor " .. mod
  if completed_spans and completed_spans[name] then
    log("skipping completed: " .. name)
    goto continue
  end
  local m = phase_begin(name)
    phase("analyze")
    local a = agent({ prompt = "Analyze " .. mod .. " for issues", schema = ANALYSIS })

    phase("refactor")
    local c = agent({ prompt = "Apply refactoring to " .. mod, schema = CHANGES })

    phase("verify")
    local v = agent({ prompt = "Verify " .. mod .. " still passes tests", schema = VERIFY })
    table.insert(results, { module = mod, ok = v.ok })
  phase_end(m)
  ::continue::
end

report({ refactored = #results, results = results })
```
```

#### 新增示例 2：动态枚举 + 3 层（大任务）

```
# Example: whole-crate refactoring (dynamic enumeration, 3-level nesting)
```lua
phase("discover subsystems")
local discover = agent({
  prompt = "Enumerate subsystems under src/ that need refactoring",
  schema = SUBSYSTEMS_SCHEMA
})

local all_results = {}
for _, sys in ipairs(discover.output.subsystems or {}) do
  local gname = "refactor " .. sys.name
  if completed_spans and completed_spans[gname] then
    log("skipping completed subsystem: " .. gname)
    goto continue
  end
  local g = phase_begin(gname)
    local mods = agent({
      prompt = "List modules in " .. sys.path .. " needing changes",
      schema = MODULES_SCHEMA
    })

    for _, mod in ipairs(mods.output.modules or {}) do
      local mname = "refactor " .. mod.name
      if completed_spans and completed_spans[mname] then
        goto skip_mod
      end
      local m = phase_begin(mname)
        phase("analyze")
        phase("change")
        phase("verify")
      phase_end(m)
      ::skip_mod::
    end
  phase_end(g)
  ::continue::
end

report({ subsystems = #all_results, results = all_results })
```
```

### 5.2 build_prompt 改动（`planner.rs:267-280`）

在 task 描述前插入分拆指导：

```rust
fn build_prompt(task: &str, fix_error: Option<&str>) -> String {
    let mut p = String::with_capacity(LUA_DSL_REFERENCE.len() + task.len() + 512);
    p.push_str(LUA_DSL_REFERENCE);
    p.push_str("\n\n# Task\n\n");
    p.push_str(task);
    p.push('\n');

    // 新增：大任务分拆引导
    p.push_str("\n# Decomposition guidance\n\n");
    p.push_str(DECOMPOSITION_HINTS);

    if let Some(err) = fix_error {
        p.push_str("\n# Your previous attempt was rejected\n\n");
        p.push_str(err);
        p.push_str("\n\nFix the script and output a corrected version.\n");
    }
    p.push_str("\nOutput ONLY one ```lua code block — no prose before or after.\n");
    p
}

const DECOMPOSITION_HINTS: &str = r##"
If this task involves multiple independent units of work (modules, files,
subsystems, documents), decompose it into phase spans using phase_begin() /
phase_end(). Each span should wrap a similar internal workflow.
For unknown or large scopes, start with an agent() call to enumerate targets,
then loop over them with one phase span per target.
When the `completed_spans` global is non-nil (resume mode), skip spans whose
name matches an existing entry.
"##;
```

### 5.3 validate_generated 改动（`planner.rs:134-140`）

新增 phase_begin/phase_end 配对检查：

```rust
fn validate_generated(script: &str) -> Result<(), String> {
    validate_script(script).map_err(|e| format!("lua syntax error: {}", e))?;
    if !script.contains("report(") {
        return Err("script must call report(...) to emit a final result".to_string());
    }
    check_span_pairing(script)?;
    Ok(())
}

/// 静态扫描 phase_begin() / phase_end() 配对。
/// 启发式文本计数，不是 AST 分析。动态循环内的 begin/end 在文本中各出现一次，
/// 但运行时 1:1 配对。只检查"有 begin 就至少有 end"，不做严格等量。
fn check_span_pairing(script: &str) -> Result<(), String> {
    let begin_count = script.matches("phase_begin(").count();
    let end_count = script.matches("phase_end(").count();
    if begin_count > 0 && end_count == 0 {
        return Err(format!(
            "script has {} phase_begin() call(s) but no phase_end() — spans must be paired",
            begin_count
        ));
    }
    Ok(())
}
```

### 5.4 Planner 改动文件清单

| 文件 | 改动类型 | 说明 |
|------|----------|------|
| `src/planner.rs` `LUA_DSL_REFERENCE` | 改 | 新增 phase_begin/phase_end 原语 + 2 个示例 + rules |
| `src/planner.rs` `build_prompt` | 改 | 插入 `DECOMPOSITION_HINTS` |
| `src/planner.rs` `validate_generated` | 改 | 新增 `check_span_pairing` |

---

## 6. Resume 机制详解

### 6.1 流程

```
首次运行:
  Runtime::execute(script)
    ├── phase_begin("重构 auth") → push span, emit PhaseSpanStarted
    │   ├── phase("分析") → agent() → journal cache key #1
    │   ├── phase("重构") → agent() → journal cache key #2
    │   └── phase("验证") → agent() → journal cache key #3
    ├── phase_end() → pop span, emit PhaseSpanDone, checkpoint 落盘
    │                  completed_spans = [{ name: "重构 auth", ... }]
    ├── phase_begin("重构 db") → push span
    │   ├── phase("分析") → agent() → journal cache key #4
    │   └── ❌ CRASH
    └── (未执行 phase_end)

Resume:
  1. 读取 checkpoint → completed_spans = ["重构 auth"]
  2. 注入 Lua 全局: completed_spans = { ["重构 auth"] = true }
  3. 重跑同一份脚本:
     ├── phase_begin("重构 auth")
     │   ├── phase("分析") → journal hit #1 → 瞬间返回
     │   ├── phase("重构") → journal hit #2 → 瞬间返回
     │   ├── phase("验证") → journal hit #3 → 瞬间返回
     │   └── phase_end() → checkpoint 发现已完成，跳过
     ├── phase_begin("重构 db")  ← 从这里开始真正执行
     │   ├── phase("分析") → journal hit #4 → 瞬间返回
     │   ├── phase("重构") → agent() → 真正调用 LLM
     │   └── ...
```

### 6.2 Resume 注入点

`service/run.rs` 的 `execute()` 函数在创建 Runtime 后注入 resume 状态：

```rust
// service/run.rs execute() 内
let completed_spans: Vec<String> = {
    if let Some(cp) = &checkpoint {
        cp.completed_spans.iter().map(|s| s.name.clone()).collect()
    } else {
        vec![]
    }
};

// Runtime 创建后注入 Lua 全局
if !completed_spans.is_empty() {
    let names_table = lua.create_table()?;
    for name in &completed_spans {
        names_table.set(name.clone(), true)?;
    }
    lua.globals().set("completed_spans", names_table)?;
}
```

> **ID 确定性**：span id 用全局递增 counter，resume 时重跑会生成不同 id。所以用 **name 匹配**（方案 C），不破坏 id 递增语义。注入的 `completed_spans` 是一个以 name 为 key 的 set table。

### 6.3 脚本侧跳过逻辑

planner 生成的脚本在循环内检查 `completed_spans`（见 §3.4 和 §5.1 示例）。`DECOMPOSITION_HINTS` 指导 LLM 生成包含此检查的脚本。

---

## 7. 测试计划

### 7.1 Runtime 单元测试（`control.rs` tests）

| 测试 | 验证点 |
|------|--------|
| `phase_begin_push_pop` | phase_begin() push，phase_end() pop，栈空 |
| `phase_begin_returns_id` | 返回的 id 递增，与 phase() 共享 counter 空间 |
| `phase_span_nested` | 3 层嵌套，depth 正确 |
| `phase_end_by_id` | 传 id 弹指定层 |
| `phase_end_empty_stack_error` | 空栈 pop 返回 RuntimeError |
| `phase_end_wrong_id_error` | 不存在的 id 返回 RuntimeError |
| `phase_parent_inside_span` | span 内 phase() 的 parent_span_id 正确 |
| `phase_parent_outside_span` | span 外 phase() 的 parent_span_id = None |
| `phase_span_emits_events` | PhaseSpanStarted + PhaseSpanDone 事件序列正确 |
| `phase_span_done_carries_parent_depth` | PhaseSpanDone 事件含 parent_id + depth |
| `parallel_span_safety` | parallel() 内 phase_begin/end 配对安全 |

### 7.2 Planner 单元测试（`planner.rs` tests）

| 测试 | 验证点 |
|------|--------|
| `test_validate_span_unpaired` | 有 phase_begin() 无 phase_end() → 拒绝 |
| `test_validate_span_paired` | 配对 → 通过 |
| `test_validate_no_span_ok` | 无 phase_begin 也通过（向后兼容） |
| `test_build_prompt_contains_decomposition_hints` | prompt 含分拆指导 |
| `test_prompt_contains_span_examples` | DSL reference 含 phase_begin 示例 |

### 7.3 State / Checkpoint 测试（`state.rs` tests）

| 测试 | 验证点 |
|------|--------|
| `checkpoint_records_span_done` | PhaseSpanDone 事件 → completed_spans 更新 |
| `checkpoint_persist_and_reload` | 落盘后重新加载，spans 完整 |
| `resume_injects_completed_spans` | resume 时注入 Lua 全局 |

### 7.4 集成测试

| 测试 | 验证点 |
|------|--------|
| `span_resume_skips_completed` | 完整流程：运行到一半 crash → resume → 跳过已完成 span |
| `nested_span_tree_events` | 3 层嵌套产生正确的树形事件序列 |
| `phase_counter_shared` | phase_begin 和 phase 共享 counter，id 全局唯一 |

---

## 8. 文档同步

| 文档 | 改动 |
|------|------|
| `docs/sdk-reference.md` | 新增 phase_begin/phase_end 签名 |
| `docs/architecture/planner.md` | 更新核心结构表、工作流程 |
| `docs/architecture/runtime.md` | 新增 phase_span_stack 机制说明 |
| `docs/dynamic-workflow-guide.md` | 新增 span 分拆模式示例 |
| `docs/dev/lua-workflow-spec.md` | DSL 规范加 phase_begin/phase_end 原语 |

---

## 9. 向后兼容性

- **现有脚本零改动**：不使用 phase_begin 的脚本完全不受影响。`phase()` 签名不变，只是新增了 `parent_span_id` 字段（`Option<u32>`，span 外为 None）。
- **事件反序列化**：`PhaseStarted` 新增 `parent_span_id` 字段加 `#[serde(default)]` 以兼容旧事件日志。
- **Checkpoint 向后兼容**：`completed_spans` 字段加 `#[serde(default)]`，旧 checkpoint 文件可正常加载。
- **validate_generated**：`check_span_pairing` 只在有 `phase_begin(` 调用时触发，无 span 的脚本不增加任何校验负担。
- **phase_counter 共享**：`phase_begin()` 和 `phase()` 消费同一个 counter，id 全局唯一。现有脚本不会因 phase_begin 的引入而改变 id 分配。
