# Luft 已知问题与技术债（Known Issues & Tech Debt）

本文记录 luft v0.3.0 当前架构层面的不足与清理债务，面向项目维护者。每条问题包含：严重程度、证据（文件路径:行号）、影响、建议修复方向。

---

## 1. Converge 对抗性共识原语被禁用

**严重程度：严重**

### 证据

- `crates/luft-runtime/src/sandbox.rs:189` — 注册处被注释：`// converge::register_converge_sdk(lua, cx)?; // temporarily disabled`
- `crates/luft-runtime/src/converge.rs:1` — 文件首行 `#![allow(dead_code)]`，整模块约 1601 行未接入，属死代码

### 影响

- `README.md:3` 与 `crates/luft/src/lib.rs` 顶层描述都把 converge 作为头牌原语宣传，但运行时根本不可用，构成"宣传与实际能力不符"
- `examples/converge-demo.lua`、`examples/deep-research.lua` 一旦调用 `converge()` 会报错（原语未注册）
- 这是最具差异化、最复杂的特性，却完全未接入

### 建议修复方向

要么重新启用并补齐测试后接入 `register_sdk`；要么在 README/lib.rs 显式标注为"未启用/实验性"并从 examples 移除，避免误导。

---

## 2. 单一 Backend 假设与非确定默认路由

**严重程度：高**

### 证据

- `crates/luft/src/builder.rs:35` — `backend: Option<Arc<dyn AgentBackend>>`，Builder 只持有单个 backend
- `crates/luft/src/builder.rs:68` — `.backend()` 方法是覆盖赋值（`self.backend = Some(Arc::new(b))`），而非追加注册
- `crates/luft-core/src/scheduler/registry.rs:47-48` — `default_backend()` 用 `self.backends.values().next()`，HashMap 迭代顺序非确定，多 backend 时默认路由不可预测（潜在 bug）

### 影响

- 整个系统假设单一 backend；多 backend 编排与路由未实现（`AgentCapabilities` 已记录 routing 字段但注释标 v0.2）
- 仅支持 ACP/OpenCode 子进程一种 backend，无原生 OpenAI/Anthropic HTTP 适配器

### 建议修复方向

Builder 支持多 backend 注册；`default_backend()` 改为确定顺序（如按 id 排序或显式默认 backend 配置）；规划原生 HTTP LLM 适配器。

---

## 3. 残留目录与命名清理

**严重程度：低**

### 证据

- `crates/workflow-cli/` 与 `crates/workflow-adapters/` — 仅含空 `src/`（内含 `.loom/` 元数据），无任何 `.rs` 文件，且不在 `Cargo.toml` workspace members 列表中，属废弃残留
- `crates/workflow-cli/` 下还散落一堆 `.profraw` 覆盖率垃圾文件（`default_*.profraw` x10）
- `migrations/20250819000001_initial.sql:1` 注释仍写 "Initial SQLite schema for Maestro"（项目曾用名 Maestro），rename 不彻底

### 影响

- 噪音与误导；`workflow-*` 命名易让贡献者误以为存在对应 crate
- 命名残留（Maestro -> luft）不彻底，影响一致性

### 建议修复方向

删除 `crates/workflow-cli`、`crates/workflow-adapters` 两个空目录及内部 `.profraw`；修正 SQL 迁移注释为 luft。

---

## 4. Runtime block_on 调用要求 Blocking Context 驱动

**严重程度：中**

### 证据

- `crates/luft-runtime/src/sdk/agent/single.rs` 附近 — Lua 同步回调内通过 tokio `Handle::block_on` 调用异步调度器
- `crates/luft-runtime/src/sandbox.rs:25-29` — 文档化约束：`Runtime::execute` 必须从 blocking context 驱动（`spawn_blocking`），否则 `block_on` 在异步工作线程内会 panic

### 影响

- 这是一个文档化的约束，但仍是一个 sharp edge / footgun：调用方稍有不慎（在 async 上下文直接调用 execute）即触发 panic
- 将同步 Lua VM 与异步调度器桥接的固有复杂度转嫁给了调用方

### 建议修复方向

在 API 层用类型/封装强制该约束（如提供 async 包装并内部 `spawn_blocking`，或在文档与函数签名处给出更显眼的告警 + 运行时检测）。

---

## 5. MCP Server 手写 JSON-RPC

**严重程度：低**

### 证据

- `crates/luft-mcp/` 手写 JSON-RPC over stdio（见 `crates/luft-mcp/src/protocol.rs`、`crates/luft-mcp/src/server.rs`），未使用 MCP SDK crate（如 rmcp）
- 协议版本固定为 "2024-11-05"（`crates/luft-mcp/src/lib.rs:109` 附近）

### 影响

- 当前仅 4 tools + 3 resources，手写尚可接受；但协议一致性完全靠手写实现保证，难以跟随 MCP 规范演进

### 建议修复方向

评估迁移到官方/社区 MCP SDK（如 rmcp），或在文档中明确标注协议版本与手写边界。

---

## 问题汇总表

| 序号 | 问题 | 严重程度 | 类别 | 状态 |
|------|------|----------|------|------|
| 1 | Converge 对抗性共识原语被禁用 | 严重 | 功能缺失/文档不符 | 待修复 |
| 2 | 单一 Backend 假设与非确定默认路由 | 高 | 架构限制/潜在 bug | 待修复 |
| 3 | 残留目录与命名清理 | 低 | 代码清理/命名一致性 | 待修复 |
| 4 | Runtime block_on 调用要求 Blocking Context 驱动 | 中 | API 设计/footgun | 待修复 |
| 5 | MCP Server 手写 JSON-RPC | 低 | 技术债/协议维护 | 待评估 |
