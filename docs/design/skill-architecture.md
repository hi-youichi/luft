# Luft Skill 架构

> **状态**: 设计阶段（草案）。§2 已实现并通过测试；§3 起（运行时安装、MCP 通道）尚未实现。
> **目标**: luft 提供一份标准化的 workflow-authoring skill，luft 自己不消费它——只负责生产内容和分发到不同消费者（loom 的 library 依赖、ACP 驱动的 agent 子进程、未来的外部 MCP client）。

---

## 1. 背景

对照 loom 发现的三个问题：

1. **loom 有一整套 skill 机制**（`agent/skill` crate：discovery/cache/guard/sync 等 + `tool_core::BuiltinSkill` + 触发词驱动的按需加载），luft 完全没有对应概念。
2. **loom 的 workflow skill 内容是独立重写的**——`workflow_skill.md`（140 行）+ 6 个 reference 文件（864 行），跟 luft 自己的 `lua_dsl_reference.md`（688 行单文件）内容同源但措辞、组织方式都不一样，两边各自维护，会漂移。
3. **luft 内部本身就有三份物理拷贝**，其中一份是死文件：

   ```
   crates/luft-planner/src/lua_dsl_reference.md   ← 源头，luft-planner/src/lib.rs 用
   crates/luft-mcp/src/lua_dsl_reference.md       ← vendor 副本，resources.rs 用（喂 workflow://schema）
   crates/luft-cli/src/lua_dsl_reference.md       ← 孤儿文件，全代码库无 .rs 引用
   ```
   三份目前逐字节相同，纯靠人工同步维护。

**设计原则**：luft 只生产内容，不管消费方怎么用；一份内容源，多条分发通道；能力边界止步于"luft 自己知道怎么写 Lua"，不管"该调用哪个具体工具名"（那是每个消费方自己的事）。

---

## 2. 数据模型与第一个实例（已实现）

```rust
// crates/luft-core/src/contract/skill.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skill {
    pub name: &'static str,
    pub description: &'static str,
    pub content: &'static str,
    pub references: &'static [(&'static str, &'static str)],
}
```

```rust
// crates/luft-planner/src/lib.rs
pub const LUA_DSL_REFERENCE: &str = include_str!("lua_dsl_reference.md");
pub const WORKFLOW_SKILL: luft_core::Skill = luft_core::Skill {
    name: "workflow",
    description: "Lua DSL reference for writing multi-agent Luft workflows",
    content: LUA_DSL_REFERENCE,
    references: &[],  // 目前为空 — 见 §3 拆分计划
};
```

字段全 `&'static str`，因为所有实例都来自编译期 `include_str!`，没有运行时构造 `Skill` 的场景。

**可达路径**：`luft` facade 的 `pub use luft_planner as planner;` 让 `luft::planner::WORKFLOW_SKILL` 对任何依赖 `luft = "0.3"` 的 crate 直接可见——这就是"library 级别对齐"：loom 的 `tool-workflow` crate 不用改协议、不用装文件，直接 `use luft::planner::WORKFLOW_SKILL;` 拿内容，自己包一层 `triggers`/`requires_tools` 成它自己的 `BuiltinSkill`。

已验证：4 个测试通过（`luft-core` 3 个 + `luft-planner` 1 个），`cargo check --workspace` 全绿。

---

## 3. 内容拆分计划（设计，未实施）

现状 `content = LUA_DSL_REFERENCE` 是全部 688 行，`references` 是空数组——完全没用上"按需加载"这个设计初衷。

**目标结构**（切分边界参照 loom 的 6 文件划分，但文字从 luft 现有原文派生，不搬 loom 重写后的措辞）：

| 留在主体（`content`，短） | 拆进 `references`（按需） |
|---|---|
| Output Format、Execution Model、Meta Table、最小骨架、Rules 精简版 | `references/architecture-header.md`（ASCII 图表符号）、`references/agent-prompts.md`（写 prompt 方法论）、`references/task-decomposition.md`、`references/adversarial-verification.md`、`references/examples.md`（三个完整例子）、可能还有 `references/dsl-reference.md`（primitives 详细表格） |

**约束**：
- `luft-planner` 自己拼给 planner LLM 的 system prompt 必须保持行为不变——建 prompt 时把主体 + 全部 references 按固定顺序拼接，等价于现在的 688 行内容。需要一个测试断言这个拼接结果覆盖原文，不能凭感觉切。
- 拆分本身应该是**纯机械**的第一步（原文一字不改，只是物理切开文件），验证过拼接一致后，文字精简是后续独立的第二步，两件事不要混在一起做。
- 顺手清理：`crates/luft-cli/src/lua_dsl_reference.md`（死文件，直接删）；`crates/luft-mcp/src/resources.rs` 的 vendor 副本改成引用拆分后的内容，把三份收敛成一份源头。

---

## 4. 三条分发通道

```
                     luft_planner::WORKFLOW_SKILL
                              │
        ┌─────────────────────┼─────────────────────┐
        ▼                     ▼                      ▼
   Library 通道           Runtime 通道            MCP 通道
   （已实现）              （设计，未实现）          （想法，未展开）
```

### 4.1 Library 通道（已实现，见 §2）

loom 直接 `use luft::planner::WORKFLOW_SKILL`，编译期拿到内容，自己包装。

### 4.2 Runtime 通道（设计，未实现）

**触发点**：`luft-adapters/src/acp_adapter.rs` 在 spawn ACP 子进程前（跟现有的 `prepare_schema_mcp` 临时文件注入同一个阶段），把 `WORKFLOW_SKILL` 的 content + references 落盘到当前 run 的 working folder 下。

**目标路径**——查证过 codex/opencode/loom 的实际源码，三家目录约定收敛成两个共享 + 一个独立，不需要 4 套各自的分支：

| 目录 | 谁认 | 来源 |
|------|------|------|
| `.agents/skills/workflow/SKILL.md` | codex、opencode | codex: [`external_agent_config.rs:752-757`](../../../codex-cli/codex-rs/app-server/src/config/external_agent_config.rs)（`home_target_skills_dir()`）；opencode: [`skill/index.ts:22-25`](../../../opencode/packages/opencode/src/skill/index.ts)（`AGENTS_EXTERNAL_DIR`） |
| `.claude/skills/workflow/SKILL.md` | Claude Code（原生）、opencode（额外兼容） | opencode: 同上文件 `CLAUDE_EXTERNAL_DIR` |

**没有 `.loom/skills` 这一项**：luft 不把 loom 当 ACP backend spawn（这条支持已移除），所以 runtime 通道压根不会对 loom 触发。loom 是从**另一个方向**消费这份内容的——它把 `luft` 当库依赖，直接拿 `luft_planner::WORKFLOW_SKILL`，即 §4.1 的 library 通道。这两条通道服务的是不同场景，不是二选一。

**按 backend 精确写入**：只写当前 spawn 的那个 backend 认的目录，用 `config.id` 判断——不是"所有路径无脑都写"。`config.id` 本来就在调用点的作用域里，不需要新增 backend 探测机制。

**生命周期**：**每次 run 现写现用，不持久安装**。run 的 working folder 本身就是临时的，run 结束这份 skill 文件的生命周期也就结束，下次 run 重新写一份全新的。因此**不需要**类似 loom `sync.rs` 那套 manifest/hash 追踪机制（防止覆盖用户本地修改）——这里没有"用户本地修改"的问题，因为压根不是持久化的东西。

### 4.3 MCP 通道（想法，未展开）

内容一旦按 §3 拆成物理文件，`luft-mcp` 可以照着已有的 `workflow://example/{name}` resource template 模式，加一个 `workflow://reference/{name}`，让外部 MCP client（Claude Code 等，走 MCP 而不是 runtime 文件安装的场景）按需单独拉某个参考文件，不用一次性吃完整个 `workflow://schema`。

---

## 5. 实施顺序建议

1. **内容拆分**（§3）——机械拆分 + 拼接一致性测试 + 清理死文件/vendor 副本。这是后面两条通道的前提，因为 `references` 字段现在是空的。
2. **Runtime 通道**（§4.2）——在 `acp_adapter.rs` 加落盘逻辑，按 `config.id` 只写当前 backend 认的目录（loom 除外，它走 library 通道）。
3. **MCP 通道**（§4.3）——可选，视需要排期，不阻塞前两步。

---

## 6. 开放问题

| # | 问题 |
|---|------|
| 1 | §3 拆分的具体切法和文字改写——谁来做这部分编辑工作，切多细 |
| 2 | Runtime 通道写入的文件要不要在 run 结束后清理，还是留着（下次 run 会覆盖，不清理理论上也无害，但会在 working folder 里留垃圾） |
| 3 | MCP 通道这次要不要一起做 |
| 4 | `luft-mcp` 的 vendor 副本这次要不要顺手切掉 |
| 5 | Claude Code 目前不是 luft 可 spawn 的 ACP backend（现有：`mock`/`opencode`/`codex`）——把它接进来是不是这次范围内的事，还是假设"以后会有"先把 `.claude/skills/` 路径写上 |

---

## 7. 相关文档

- 数据结构实现：[`crates/luft-core/src/contract/skill.rs`](../../crates/luft-core/src/contract/skill.rs)
- 第一个实例：[`crates/luft-planner/src/lib.rs`](../../crates/luft-planner/src/lib.rs)
- ACP 注入现有模式参照：[`crates/luft-adapters/src/acp_adapter.rs`](../../crates/luft-adapters/src/acp_adapter.rs)（`prepare_schema_mcp`）
- MCP 工具对齐 loom 的另一份设计（不同话题，同样待实现）：[`docs/design/mcp-loom-alignment.md`](./mcp-loom-alignment.md)
- 工具执行核心重构草案：[`docs/design/tool-registry.md`](./tool-registry.md)
