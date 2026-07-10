# ACP (Agent Client Protocol) 升级指南

## 背景

opencode v1.17.3 发送了 `usage_update` 类型的 `session/update` 通知，用于报告上下文窗口使用量和累计费用。Luft 当前依赖的 `agent-client-protocol` **0.11.1**（schema **0.12.0**）不支持该变体，导致反序列化报错：

```
WARN Handler errored: unknown variant `usage_update`, expected one of
  `user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk`,
  `tool_call`, `tool_call_update`, `plan`, `available_commands_update`,
  `current_mode_update`, `config_option_update`, `session_info_update`
```

该错误由 opencode 端日志输出，不影响 Luft 运行，但 token 统计不完整。

## 当前版本

| Crate | 版本 |
|---|---|
| `agent-client-protocol` | 0.11.1 |
| `agent-client-protocol-schema` | 0.12.0 (lock) |
| `agent-client-protocol-derive` | (lock) |

## 目标版本

| Crate | 版本 | 主要变更 |
|---|---|---|
| `agent-client-protocol` | 0.14.0 | |
| `agent-client-protocol-schema` | 0.13.6 | 新增 `UsageUpdate`、`SessionClose`、`SessionResume` 等 |

## 版本间变更摘要

### 0.11 → 0.12

- Stabilize `SessionInfoUpdate` 变体
- Stabilize `session/list` 方法
- Unstable: `session/close`、`session/stop` 重命名

### 0.12 → 0.13

- Stabilize `session/close`
- Stabilize `session/resume`
- 新增 `UsageUpdate` 变体（解决本问题）
- Unstable: MCP-over-ACP 实验性消息类型
- Unstable: v2 Schema 脚手架
- Schema 模块重组到 `v1` module（需检查 import 路径）

### 0.13 → 0.14

- 继续稳定化 MCP-over-ACP
- 内部依赖更新

## 升级步骤

### 第 1 步：更新 Cargo.toml

```toml
# Cargo.toml
# 旧
agent-client-protocol = "0.11.1"

# 新
agent-client-protocol = "0.14"
```

### 第 2 步：更新依赖

```bash
cargo update agent-client-protocol
cargo build 2>&1 | head -50
```

### 第 3 步：处理编译错误

预期需要修改的文件：

#### 3.1 Import 路径变更（0.13 重组了 v1 module）

**文件：** `src/adapters/acp_adapter.rs:26-31`

```rust
// 可能需要从 v1 子模块导入
use agent_client_protocol::schema::v1::{...};
// 或
use agent_client_protocol::schema::{...};  // 如果保留了 re-export
```

**文件：** `src/adapters/update_mapper.rs:10`

```rust
use agent_client_protocol::schema::SessionUpdate;
// 可能变为
use agent_client_protocol::schema::v1::SessionUpdate;
```

**文件：** `src/adapters/permission.rs:9`

```rust
use agent_client_protocol::schema::RequestPermissionRequest;
```

#### 3.2 新增 `UsageUpdate` 处理

**文件：** `src/adapters/update_mapper.rs`

`SessionUpdate` enum 新增了 `UsageUpdate` 变体。由于 enum 标记为 `#[non_exhaustive]`，现有 match 必须有通配分支。检查当前代码：

```rust
// 当前代码使用了哪些 SessionUpdate 变体
SessionUpdate::AgentMessageChunk(chunk) => { ... }
SessionUpdate::AgentThoughtChunk(chunk) => { ... }
SessionUpdate::ToolCall(tc) => { ... }
SessionUpdate::ToolCallUpdate(u) => { ... }
SessionUpdate::Plan(plan) => { ... }
```

如果 match 没有 `_ => {}` 通配分支，需要添加，否则编译失败。

新增 `UsageUpdate` 的显式处理（可选，提升 token 统计准确性）：

```rust
SessionUpdate::UsageUpdate(u) => {
    let usage = TokenUsage {
        input: u.used as u64,
        output: 0,
        cache_read: 0,
        cache_write: 0,
    };
    *acc.tokens.lock().unwrap() = usage;
}
```

#### 3.3 `extract_usage` 函数更新

**文件：** `src/adapters/update_mapper.rs:129`

当前 `extract_usage` 从 `SessionUpdate` JSON 手动解析 token 字段。升级后可以直接从 `UsageUpdate` 变体获取准确数据。

### 第 4 步：运行测试

```bash
cargo test 2>&1
```

关注以下测试：
- `adapters::update_mapper::tests::*` — SessionUpdate 解析测试
- `adapters::acp_adapter::tests::*` — ACP 协议测试
- `adapters::permission::tests::*` — 权限请求解析

### 第 5 步：集成验证

用 opencode backend 跑一个简单案例，确认不再出现 `usage_update` 错误：

```bash
cargo run -- run -w examples/hello.lua -b opencode \
    --log .luft/example_logs/hello-acp.jsonl --log-format jsonl

# 检查日志中是否有 UsageUpdate 事件
grep "usage_update" .luft/example_logs/hello-acp.jsonl
```

### 第 6 步：更新断言脚本

如果 `UsageUpdate` 作为新事件类型暴露到 `AgentEvent`，需要在 `scripts/run_examples.sh` 中更新 span 配对检查。

## 风险评估

| 风险 | 等级 | 说明 |
|---|---|---|
| Import 路径变更 | 高 | 0.13 可能重组了 schema module 到 v1 子路径 |
| `#[non_exhaustive]` match | 中 | 新增变体会导致缺少通配分支的 match 编译失败 |
| API 行为变更 | 低 | 现有变体的字段类型可能微调 |
| opencode 兼容性 | 低 | 0.14 是当前最新，与 opencode 1.17.3 对齐 |

## 回滚方案

如果升级出现不可解决的问题：

```toml
# Cargo.toml 回滚到
agent-client-protocol = "0.11.1"
```

```bash
cargo update agent-client-protocol
cargo build
```
