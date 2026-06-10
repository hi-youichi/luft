# Planner 模块开发方案

> **文档状态**：开发中（阶段一~四已完成，2025-07）
> **实现状态**：✅ 阶段一(类型) ✅ 阶段二(Prompt+Schema) ✅ 阶段三(Meta提取) ✅ 阶段四(验证器)
> **待完成**：TUI 集成（`src/tui` 消费 `workflow.phases`）
> **测试**：29/29 通过
> **目标**：通过 `meta + script` 双字段输出，解决 TUI 结构展示与 token 成本的矛盾
> **核心决策**：Agent 输出 `{ meta: { phases }, script }` — meta 声明式展示，script 可执行编排
> **依据**：`src/planner.rs` 实现 + Claude Code Dynamic Workflows meta 块先例

---

## 1. 背景与问题

### 1.1 当前实现（已重构：`meta + script` 双字段模式）

**文件**：`src/planner.rs` + `src/planner/types.rs`（已实现）

```rust
// src/planner.rs — plan_workflow（已重构）
pub async fn plan_workflow(
    task: &str,
    backend: Arc<dyn AgentBackend>,
    cfg: &PlannerConfig,  // 含 use_structured_output, validation_enabled
) -> Result<PlannedWorkflow, PlannerError> {
    // 1. build_prompt():
    //    - structured=true: LUA_DSL_REFERENCE + META_FORMAT_SPEC + Few-shot 示例
    //    - structured=false: 仅 LUA_DSL_REFERENCE（回退模式）
    // 2. backend.run()（output_schema = Some(planner_output_schema())）
    // 3. fence scanner 提取 ```lua 代码块
    // 4. validate_plan(script, structured) — 5 层验证
    //    - Layer 1: mlua 语法验证
    //    - Layer 2: 必须有 report() 调用
    //    - Layer 3: mlua 提取 meta（仅执行顶层）
    //    - Layer 4: meta-script 交叉验证警告
    //    - Layer 5: fan-out 计数警告
    // 5. 重试循环：最多 max_retries 次
}

// 新增 PlannedWorkflow（含 phases + reasoning）
// src/planner/types.rs
pub struct PlannedWorkflow {
    pub phases: Vec<PhaseMeta>,  // 从 meta 提取
    pub script: String,          // 可执行 Lua 代码
    pub reasoning: String,        // 规划理由
}

// 新增 PlannerConfig 字段
pub struct PlannerConfig {
    pub planner_model: Option<String>,   // None = backend 默认模型
    pub max_retries: usize,             // 默认 3
    pub use_structured_output: bool,     // 默认 true
    pub validation_enabled: bool,        // 默认 true
    pub interactive_confirm: bool,       // 默认 false
}

// 新增类型（src/planner/types.rs）
pub struct PlanOutput       { meta: PlanMeta, script: String }
pub struct PlanMeta         { phases: Vec<PhaseMeta>, reasoning: String }
pub struct PhaseMeta        { label, detail, agents: u32, depends_on: Vec<u32> }
pub enum PlanningState      { Thinking, Generating, Validating, Done, Error(String) }
pub struct ValidationResult { errors: Vec<String>, warnings: Vec<String> }
```

**已有能力**：
- ✅ 重试机制（最多 `max_retries` 次），失败时将前次验证错误反馈给 agent 自修复
- ✅ 真正的 Lua 语法验证（通过 `mlua::Lua::load()`），而非简单字符串匹配
- ✅ 手写 fence scanner，不依赖 regex，支持跳过非 Lua 代码块
- ✅ TUI 已有 `planning◐` spinner + 尝试计数显示

**问题**：
| 问题 | 影响 | 根因 |
|------|------|------|
| Agent 输出格式不一致 | 规划失败率高 | 依赖文本解析，无结构化约束 |
| 无法渐进式确认 | 用户信任度低 | 单次调用，无交互机制 |
| 规划进度不透明 | TUI 显示不够精细 | 只显示 spinner + 尝试次数，不知道 Agent 在思考什么 |
| 错误恢复粒度粗 | 重试效果不稳定 | 只能反馈验证错误文本，无法针对性修复 |
| 生成的脚本质量不可控 | 脚本可能不遵循最佳实践 | 仅验证语法+report()，不检查结构合理性 |

### 1.2 重构目标

**核心思路**：保持 "Agent 编译器" 哲学（NL → Lua DSL），通过 `meta` + `script` 双字段输出同时解决 TUI 展示和 token 成本问题。

> **已验证的方向**：Claude Code Dynamic Workflows 的脚本包含 `meta` 块（声明 phase 信息）+
> 脚本体。Maestro 采用同样模式：Agent 一次输出，meta 声明在 `meta = {...}` table，script 封装在 `main()` 中。

```
NL 输入 ──▶ 增强 Prompt ──▶ Agent 生成 { meta, script } ──▶ 多层验证 ──▶ Runtime 执行
               │                    │                           │
               │  DSL Reference     │  meta: 声明式 phase 树      │  meta 直接用于 TUI
               │  + 4 个 Few-shot   │  仅描述结构，不描述细节     │  script 执行编排
               │  + meta 格式规范   │  script: 可执行 Lua 代码    │  mlua 语法验证
               │                    │                           │  + report() 检查
               │                    │                           │
               └── 重试反馈：验证错误回传 ───────────────────────┘
```

**关键决策**：Agent 输出 `{ meta, script }` 两个字段。

| 字段 | 用途 | 格式 | 消费方 |
|------|------|------|--------|
| `meta.phases` | TUI 展示 + 进度追踪 | 声明式描述（只含 label/detail/depends_on） | TUI / planner |
| `script` | 实际执行 | 完整 Lua 编排代码 | Runtime |

理由：
1. **TUI 需要结构**：不能靠反向解析 Lua（嵌套 `parallel`/`pipeline` 无法正则匹配）
2. **不能全量 JSON IR**：token +78% 太重，且引入的 PhaseSpec 需与 Lua DSL 语义同步
3. **meta 是最小结构化开销**：只描述 phase 层级（label/detail/agent 计数），不描述每个 agent 的 prompt/tools/model——这些仍在 script 里
4. **Claude Code 先例**：Workflow 脚本的 `meta = { phases: [...] }` 正是这个模式
5. **不一致时 script 为准**：meta 是声明，script 是真理源。TUI 如果发现矛盾，以 script 实际执行为准

---

## 2. 架构设计

### 2.1 核心数据类型

```rust
// src/planner/types.rs（已实现）

/// Agent 输出的规划结果
///
/// 对应 Agent 返回的 JSON：{ meta: { phases: [...] }, script: "..." }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanOutput {
    pub meta: PlanMeta,
    pub script: String,
}

/// 声明式 phase 描述（仅用于 TUI 展示，不参与执行）
///
/// 注意：这是简化描述，不代表完整 Lua DSL 语义。
/// script 字段中的实际 Lua 代码才是编排的真理源。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanMeta {
    pub phases: Vec<PhaseMeta>,
    #[serde(default)]
    pub reasoning: String,
}

/// 单个 phase 的声明式描述
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseMeta {
    pub label: String,              // TUI 显示名，如 "discovery"
    pub detail: String,             // 一行描述，如 "扫描所有函数定义"
    #[serde(default)]
    pub agents: u32,                // 预计启动的 agent 数量（用于 TUI 计数显示）
    #[serde(default)]
    pub depends_on: Vec<u32>,       // 依赖的 phase 索引（0-based），如 [0] 表示依赖第一个 phase
}

/// 内部规划结果
#[derive(Debug, Clone)]
pub struct PlannedWorkflow {
    pub phases: Vec<PhaseMeta>,      // 从 meta 直接获取
    pub script: String,              // 可执行 Lua 代码
    pub reasoning: String,           // 规划理由
}

/// 规划状态（用于流式进度）
#[derive(Debug, Clone)]
pub enum PlanningState {
    Thinking,
    Generating,
    Validating,
    Done(PlannedWorkflow),
    Error(String),
}

/// 验证结果（errors=硬失败，warnings=软警告）
#[derive(Debug, Default)]
pub struct ValidationResult {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}
```

### 2.2 PlannerConfig 扩展

```rust
// src/planner.rs 中的 PlannerConfig（已扩展）
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    pub planner_model: Option<String>,   // None = backend 默认模型
    pub max_retries: usize,              // 默认 3
    pub use_structured_output: bool,     // 默认 true，启用 output_schema
    pub validation_enabled: bool,        // 默认 true，启用多层验证
    pub interactive_confirm: bool,       // 默认 false，TUI 模式为 true
}
```

---

## 3. 实现方案

### 3.1 阶段一：数据结构定义 ✅ 已实现

**文件**：`src/planner/types.rs`（新建）

实现了完整的类型定义：`PlanOutput`、`PlanMeta`、`PhaseMeta`、`PlannedWorkflow`、`PlanningState`、`ValidationResult`。

包含完整的单元测试（JSON 反序列化、默认值处理、类型转换）。

### 3.2 阶段二：Prompt 增强 ✅ 已实现

**文件**：`src/planner.rs`（`LUA_DSL_REFERENCE` + `META_FORMAT_SPEC` + `PLANNER_FEW_SHOT_EXAMPLES` 常量）

**核心决策**：读/写分离——顶层声明 `meta = {...}` + `function main() ... end` 是纯数据定义和函数注册，无副作用；`main()` 才是编排入口。

**实际实现**（`src/planner.rs`）：
- `LUA_DSL_REFERENCE`：完整的 DSL 参考文档（agent/parallel/pipeline/converge/phase/log/report 等原语）
- `META_FORMAT_SPEC`：meta 格式规范 + 关键规则说明
- `PLANNER_FEW_SHOT_EXAMPLES`：3 个完整示例（Fan-out、Pipeline、Converge）
- `planner_output_schema()`：JSON Schema（runtime 构造，因 serde_json::json! 非 const）

**Prompt 格式示例**（structured=true）：
```
# DSL Reference
...（agent/parallel/pipeline/converge/phase/log/budget/workflow/report 原语）...

# Output format: meta + script
（meta 格式规范 + 3 个 Few-shot 示例）

# Task
用户任务描述

Output ONLY one ```lua code block ...
```

### 3.3 阶段三：Meta 提取与解析 ✅ 已实现

**文件**：`src/planner.rs` — `extract_meta()` + `extract_reasoning()` 函数

**安全提取机制**：
```rust
/// 执行顶层代码（仅赋值 + 函数定义），读取 meta 全局。
/// main() 未被调用，因此不会触发任何 agent()/parallel()/... 调用。
fn extract_meta(script: &str) -> Option<Vec<PhaseMeta>> {
    let lua = mlua::Lua::new();
    // 执行顶层代码：meta = {...} + function main() ... end
    // 不调用 main()，所以 SDK 原语不会被执行
    if lua.load(script).exec().is_err() { return None; }
    let meta_table: mlua::Table = lua.globals().get("meta")?;
    let phases: Vec<mlua::Table> = meta_table.get("phases")?;
    // 逐 phase 提取 label/detail/agents/depends_on
    Some(...)
}
```

> **为什么安全**：`lua.load(script).exec()` 执行顶层代码——只有 `meta = {...}` 和
> `function main() ... end`，不调用 `main()`。所有 SDK 调用（`agent`/`parallel`/`pipeline`）
> 在 `main()` 内，不会被执行。

### 3.4 阶段四：验证器增强 ✅ 已实现

**文件**：`src/planner.rs` — `validate_plan()` + `validate_legacy()` + `ValidationResult`

**5 层验证**：
1. **Layer 1**：mlua 语法验证（`runtime::validate_script()`）
2. **Layer 2**：必须有 `report()` 调用
3. **Layer 3**：mlua 提取 meta（仅执行顶层，structured 模式）
4. **Layer 4**：meta-script 交叉验证警告（phase label 存在于 script 中）
5. **Layer 5**：fan-out 计数警告（>32 agents）

---

## 4. TUI 集成（待实现）

### 4.1 规划进度显示

> **关键简化**：TUI 直接消费 `workflow.phases`，不需要反向解析 Lua。

```rust
// TuiEvent 扩展
enum TuiEvent {
    Planned(PlannedWorkflow),    // 规划完成，携带 meta + script
    PlanningFailed(String),      // 规划失败
}

// TUI 渲染：直接遍历 workflow.phases
fn render_plan_tree(phases: &[PhaseMeta]) {
    for (i, phase) in phases.iter().enumerate() {
        let deps = if phase.depends_on.is_empty() {
            String::new()
        } else {
            format!(" ← phase_{}", phase.depends_on.iter().join(", "))
        };
        println!("  phase(\"{}\", {}){} · {} agents · {}",
            phase.label, i, deps, phase.agents, phase.detail);
    }
}
```

**TUI 显示示例**：
```
maestro ─ run abc12345  ⏱ 5.2s    ◐ running
task: 分析代码库找出未使用函数
─────────────────────────────────────────
  ◐ 规划完成

  phase("discovery", 0) · 1 agent
    └─ 扫描代码库中的函数定义

  phase("analysis", 1) ← phase_0 · 5 agents
    └─ 分析每个函数的使用情况

  理由: 先发现所有函数再分析使用情况，analysis 依赖 discovery 的结果
─────────────────────────────────────────
tokens 12.3k↑ 0.8k↓  ·  conc 1/8  ·  follow ●
↑↓ 导航  Enter 展开  f 跟随  q 退出
```

---

## 5. 成本分析

### 5.1 Token 消耗估算

| 场景 | 当前（文本提取） | meta + script 方案 | JSON IR + generator（不采用） |
|------|-----------------|-------------------|------------------------------|
| 输入 token | ~1.5K（LUA_DSL_REFERENCE） | ~1.8K（+ meta 格式说明） | ~2.0K（含 JSON schema） |
| 输出 token（典型） | ~0.3K（Lua 脚本） | ~0.6K（meta ~0.2K + script ~0.4K） | ~1.2K（JSON phases + agents） |
| 总 token / 次 | ~1.8K | ~2.4K（**+33%**） | ~3.2K（+78%） |
| 重试成本（3 次） | ~5.4K | ~7.2K | ~9.6K |

---

## 6. 测试策略

### 6.1 已实现的测试（29/29 通过）

| 测试 | 覆盖内容 |
|------|---------|
| `test_plan_output_deserialization` | JSON → PlanOutput 反序列化 |
| `test_phase_meta_defaults` | agents/depends_on 默认值 |
| `test_planned_workflow_from_plan_output` | PlanOutput → PlannedWorkflow |
| `test_planned_workflow_from_script` | 回退模式 |
| `test_validation_result_valid` | ValidationResult::new |
| `test_validation_result_errors` | 硬错误 |
| `test_validation_result_warnings` | 软警告 |
| `test_validation_result_display` | Display trait |
| `test_planning_state_display` | PlanningState Display |
| `test_plan_extracts_and_validates_script` | 端到端规划 |
| `test_plan_retries_on_invalid_then_fails` | 重试耗尽 |
| `test_plan_rejects_missing_report` | 缺少 report() |
| `test_extract_script_fenced_bare_and_object` | 代码块提取 |
| `test_extract_skips_non_lua_block` | 跳过非 Lua 块 |
| `test_extract_meta_valid_script` | mlua 提取 meta |
| `test_extract_meta_minimal` | 最小字段 |
| `test_extract_meta_missing_returns_none` | 缺失 meta |
| `test_extract_meta_invalid_lua_returns_none` | 无效 Lua |
| `test_extract_reasoning` | reasoning 提取 |
| `test_extract_reasoning_missing` | 缺失 reasoning |
| `test_validate_plan_with_meta_consistency_warning` | 交叉验证警告 |
| `test_validate_plan_missing_report_error` | 硬错误 |
| `test_validate_plan_syntax_error` | 语法错误 |
| `test_planner_output_schema_valid` | Schema 验证 |
| `test_planner_output_schema_missing_script` | Schema 缺失字段 |
| `test_planner_output_schema_missing_meta_phases` | Schema 缺失 phases |
| `test_build_prompt_adds_meta_spec_when_structured` | Prompt 构建 |
| `test_build_prompt_omits_meta_spec_when_legacy` | 回退模式 Prompt |
| `test_build_prompt_includes_fix_error` | 错误回传 |

---

## 7. 实施计划

### 7.1 阶段依赖图 + 状态

```
阶段一 ✅   阶段二 ✅   阶段三 ✅   阶段四 ✅
  │            │            │            │
  └────────────┴────────────┴──── TUI 集成 ⬜（待实现）
                                          │
                                          ▼
                                     测试验收
```

### 7.2 详细时间表

| 阶段 | 工作内容 | 预计时间 | 状态 | 风险 |
|------|---------|---------|------|------|
| 阶段一 | PlanOutput/PhaseMeta 类型定义 + 测试 | 1 天 | ✅ 已完成 | 低 |
| 阶段二 | Prompt 增强（meta 格式 + Few-shot 示例） | 2-3 天 | ✅ 已完成 | 中（prompt engineering 迭代） |
| 阶段三 | Meta 提取器 + `build_workflow` 集成 | 1-2 天 | ✅ 已完成 | 低（纯 mlua 解析） |
| 阶段四 | 验证器增强（meta-script 一致性） | 1 天 | ✅ 已完成 | 低 |
| TUI 集成 | 直接渲染 `workflow.phases` | 1 天 | ⬜ 待实现 | 低 |
| 测试验收 | 端到端测试 | 1 天 | ⬜ 待实现 | 低 |

**已完成测试**：29/29 单元测试通过（`cargo test planner`）

---

## 8. 回退策略

### 8.1 功能开关

```rust
// src/planner.rs
pub async fn plan_workflow(...) {
    // 结构化和回退路径共用同一函数
    // 通过 cfg.use_structured_output 控制行为
}
```

`use_structured_output = false` 时：跳过 meta 提取，使用旧文本提取逻辑。

### 8.2 渐进式迁移

1. **第一阶段**：添加 `use_structured_output` 开关（默认 `true`），已实现
2. **第二阶段**（观察期）：收集指标 → 新路径成功率 ≥ 旧路径 95% 且 token 增量 < 20% 才视为稳定
3. **第三阶段**（切换默认值）：`use_structured_output` 默认 `true`
4. **第四阶段**（清理）：移除旧路径

---

## 9. 附录

### A. 相关文件清单

| 文件 | 作用 | 状态 |
|------|------|------|
| `src/planner.rs` | 核心规划逻辑 | ✅ 已增强 |
| `src/planner/types.rs` | PlanOutput/PhaseMeta/PlannedWorkflow/ValidationResult 类型 | ✅ 新建 |
| `src/core/contract/backend.rs` | AgentBackend trait（output_schema 已存在） | ✅ 参考 |
| `src/core/scheduler/mod.rs` | Scheduler（schema validation 已存在） | ✅ 参考 |
| `src/runtime/sandbox.rs` | Runtime（validate_script 已存在） | ✅ 参考 |
| `src/tui/mod.rs` | TUI 集成（渲染 workflow.phases） | ⬜ 待实现 |

> **实际实现差异**：未拆分 `config.rs`/`validator.rs`——验证逻辑和配置字段保留在 `planner.rs` 中。
> 删除了 `parser.rs`（不需要 Lua→PhaseSpec 反向解析，mlua 直接提取 meta）。

### B. Meta 声明格式规范

Agent 输出的 Lua 脚本顶部必须以 `meta = {...}` 声明 phase 结构：

```lua
meta = {
    phases = {
        {
            label = "discovery",
            detail = "扫描代码库",
            agents = 3,
            depends_on = {}
        }
    },
    reasoning = "先扫描再分析的线性流程"
}

function main()
    phase("discovery", 0)
    ...
    report({...})
end
```

**Lua table 约束**：
- `phases`：必填，`table` (array)
- `phases[i].label`：必填，`string`，与 `phase()` 调用对应
- `phases[i].detail`：必填，`string`，一行描述
- `phases[i].agents`：可选，`integer`，默认 0
- `phases[i].depends_on`：可选，`table` (array)，0-based phase 索引
- `reasoning`：可选，`string`，默认 ""

### C. 行业对比

| 系统 | 规划方式 | 输出格式 | TUI 展示方式 | 适用场景 |
|------|---------|---------|-------------|---------|
| **Claude Code Dynamic Workflows** | LLM 写 JS 编排脚本 | JS + meta block | meta 声明式 | 大规模 fan-out、adversarial 验证 |
| Claude Code（默认 loop） | while(tool_use) 循环 | Tool Call JSON | 无（实时流） | 交互式编码任务 |
| Aider | architect 模式 | Markdown plan + 代码 | Markdown 渲染 | 架构级代码变更 |
| **Maestro（重构后）** | Agent 编译 NL → Lua 文件（`meta = {...}` + `main()`） | `.lua` 文件 | meta.phases 直接渲染 | 多 Agent 编排 + 结构化 TUI |

### D. Claude Code Dynamic Workflows 对比

| 维度 | Claude Code | Maestro（重构后） | 一致性 |
|------|------------|---------------|--------|
| **meta 用途** | TUI 进度展示 | TUI 规划树展示 | ✅ |
| **meta 内容** | phase 名称 + 描述 | label + detail + agents + depends_on | ✅ |
| **script 执行** | JS runtime 直接执行 | Lua sandbox 执行 | ✅ |
| **不一致处理** | script 为准 | script 为准（meta 仅展示） | ✅ |
| **Planner 独立性** | 无独立 planner | 独立 `plan_workflow()` | △ 差异点 |

**核心洞察**：`meta + script` 双字段方案在两个系统中独立演化出相同模式——
这强烈表明它是解决"编排代码执行"与"用户可观测性"矛盾的正确抽象。

---

*文档版本：v1.1.0（开发中）*
*最后更新：2025-07（实现阶段一~四）*
*实现状态：阶段一二三四 ✅ 已完成，TUI 集成 ⬜ 待实现*
*测试状态：29/29 单元测试通过*
