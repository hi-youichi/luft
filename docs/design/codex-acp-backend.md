# Codex ACP 后端接入设计

> **状态**：提案
>
> **目标**：将 [`@agentclientprotocol/codex-acp`](https://github.com/agentclientprotocol/codex-acp) 作为 Luft 的一等 ACP 后端接入，同时保留现有 OpenCode 与 Loom 后端。
>
> **相关代码**：`crates/luft-adapters/src/acp_adapter.rs`、`crates/luft-cli/src/backend.rs`、`crates/luft-cli/src/config.rs`

---

## 1. 背景与目标

Luft 已经是一个 ACP（Agent Client Protocol）客户端：`AcpAdapter` 启动一个 stdio 子进程，完成 `initialize → session/new → session/prompt`，接收 `session/update` 与权限请求，并投射为 Luft 的 `AgentResult` 和 `AgentEvent`。

`codex-acp` 是一个 stdio ACP agent server。它在内部启动 Codex App Server，并把 ACP 请求及事件映射到 Codex。因此无需新建第二套传输层或直接依赖 Codex 私有协议：Luft 应复用既有 `AcpAdapter`，仅增加 Codex 的显式后端描述、认证与可观测性支持。

本设计目标：

- 提供明确的 `codex` backend，不伪装为 `opencode`。
- 首期可通过 `npx` 零全局安装启动；稳定部署可无缝切到全局 `codex-acp` 二进制。
- 安全传递认证与 sandbox 配置，不将密钥写入配置文件或工作流。
- 优先消费标准 ACP 事件；保留 Codex 扩展元数据，避免协议升级破坏兼容性。
- 保持 OpenCode 与 Loom 的已有行为不变。

非目标：

- 不直接嵌入 Codex App Server 或复制 `codex-acp` 实现。
- P0 不实现会话复用；维持当前“一次 `AgentBackend::run` = 一个 ACP session”的隔离模型。
- P0 不把 Codex 内部 subagent 纳入 Luft scheduler，避免双重调度、预算重复计算和取消语义冲突。

---

## 2. 架构

```text
Lua workflow
  │
  ▼
Luft scheduler
  │ AgentBackend::run
  ▼
AcpAdapter (ACP client)
  ├── opencode acp
  ├── loom-acp
  └── codex-acp (ACP agent server)
        └── Codex App Server
              └── Codex
```

三个真实后端都通过同一个 ACP client 生命周期运行：

1. 启动 stdio 子进程。
2. `initialize`，协商协议、能力与认证方法。
3. 必要时 `authenticate`。
4. `session/new`，传入 workspace、MCP servers 与配置。
5. 发送 `session/prompt`，并消费流式 `session/update`。
6. 根据 `session/request_permission` 执行 Luft `ToolPolicy`。
7. 收集 stop reason、文本、structured output 与 token usage。

ACP 标准覆盖消息、推理、tool call、plan、权限、session mode、config option、slash command 和 MCP 注入。Codex 特有数据通过标准 tool call 或 `_meta.codex.*` 扩展携带；未知扩展必须原样保留为 `AcpRaw`。

---

## 3. 后端与配置模型

### 3.1 后端 ID

已知后端应为：

| ID | 默认命令 | ACP 参数 | 说明 |
|---|---|---|---|
| `mock` | — | — | 确定性测试后端 |
| `opencode` | `opencode` | `acp` | 现有后端 |
| `loom-acp` | `loom-acp` | 无 | 现有后端 |
| `codex` | 平台相关 | 平台相关 | 新增 Codex ACP 后端 |

`codex` 必须拥有独立配置段。当前共用的 `[backend.acp]` 只能描述一个二进制，若用它覆盖为 Codex，将使 OpenCode/Loom 的启动、探测与日志被错误配置。

### 3.2 P0 配置

用户级配置文件位于 `dirs::config_dir()/luft/config.toml`。Windows 的零安装启动配置如下：

```toml
[backend]
default = "codex"

[backend.codex_acp]
command = "npx.cmd"
args = ["-y", "@agentclientprotocol/codex-acp"]
connect_timeout_secs = 30
idle_timeout_secs = 900
emit_raw_events = true
initial_agent_mode = "agent"
```

Linux/macOS 仅将 `command` 改为 `npx`：

```toml
[backend.codex_acp]
command = "npx"
args = ["-y", "@agentclientprotocol/codex-acp"]
```

`npx` 首次运行会下载并缓存包；由于 Luft 每个 agent task 会创建一个 ACP 子进程，高并发或长期运行时建议改用全局安装：

```bash
npm install -g @agentclientprotocol/codex-acp
```

随后将 `command` 改为 `codex-acp`，其余 Luft 配置保持不变。

### 3.3 配置结构建议

将现有单一 `AcpConfigOverride` 演进为按后端独立的配置，例如：

```rust
pub struct AcpBackendOverride {
    pub command: Option<PathBuf>,
    pub args: Option<Vec<String>>,
    pub connect_timeout_secs: Option<u64>,
    pub idle_timeout_secs: Option<u64>,
    pub emit_raw_events: Option<bool>,
    pub inherit_env: Option<Vec<String>>,
    pub env: Option<BTreeMap<String, String>>,
}
```

其中：

- `inherit_env` 仅保存变量名，值始终从父进程读取；用于有意传递认证变量。
- `env` 仅允许非敏感运行时选项，如 `INITIAL_AGENT_MODE`、`APP_SERVER_LOGS` 和 `NO_BROWSER`。
- 禁止把 `CODEX_API_KEY`、`OPENAI_API_KEY` 或 token 值序列化进 TOML。

`log_level` 不应作为所有 ACP backend 的通用 CLI 参数继续追加。现有 adapter 会附加 `--log-level`，而 `npx` 与 `codex-acp` 未必接受该参数。Luft 自身使用 tracing；Codex adapter 的日志使用 `APP_SERVER_LOGS` 环境变量。

---

## 4. 认证与安全边界

`codex-acp` 在 initialize 中声明认证方法，支持 ChatGPT 登录、`CODEX_API_KEY` / `OPENAI_API_KEY`，以及兼容网关。ACP 的标准流程允许 client 在 initialize 后发送 `authenticate`。

当前 `AcpAdapter` 会调用 `env_clear()`，默认只转发启动所需的操作系统变量；这对可重复性有益，但意味着 API key 默认不会到达 Codex。因此 P0/P1 必须明确处理以下策略：

| 场景 | 策略 |
|---|---|
| 本机已登录 Codex | 允许 adapter 通过用户目录读取已有状态；不复制凭据 |
| API key | 用户在运行环境设置 key；`inherit_env` 显式允许 `CODEX_API_KEY` 或 `OPENAI_API_KEY` |
| CI / 无浏览器环境 | 设置 `NO_BROWSER=1`；只使用 API key 或预置的非交互认证 |
| 自定义网关 | 仅在明确配置并通过 capability 协商后启用 |

建议默认 `INITIAL_AGENT_MODE=agent`，而不是 `agent-full-access`。同时，Luft 在存在 `ToolPolicy` 时应严格执行命令、编辑和 MCP allowlist；无 policy 自动批准全部权限只适合受信任的本地开发，不适合共享或 CI 环境。

---

## 5. Codex 事件映射

### 5.1 标准 ACP：P0/P1 直接复用

| Codex 活动 | ACP 表达 | Luft 处理 |
|---|---|---|
| 回答、推理 | `session/update` message / thought chunk | `ProgressDelta::Message`，累积最终文本 |
| shell、文件、MCP、终端 | tool call / tool call update | 工具进度、文件编辑、原始事件 |
| 计划 | `session/update` plan | 原始事件；后续可升为结构化 plan |
| token usage | usage update 或结束信息 | `TokenUsage` |
| 权限 | `session/request_permission` | `ToolPolicy` 决策 |
| model、reasoning、approval、sandbox | session config option / session mode | 初始化后按能力协商设置 |
| MCP server | `session/new.mcp_servers` | 复用 Luft structured-output MCP server |

### 5.2 Codex 扩展：P2 增量映射

| 能力 | 传输方式 | P2 行为 |
|---|---|---|
| Codex subagent | 标准 tool call + `_meta.codex.subagent` | 新增可选 `AgentEvent::CodexSubagent`；不创建 Luft scheduler task |
| review | tool call / tool update / slash command | 新增 review 进度与最终摘要 |
| web search、image generation、image view | tool call / update | 记录为结构化工具事件，必要时保留 metadata |
| 未知的 Codex 新事件 | `_meta` 或未知 payload | 始终投射为 `AcpRaw`，不造成 protocol error |

对 `_meta.codex.*` 的读取必须是可选的、版本容忍的：只依赖稳定字段，缺失或无法反序列化时降级到原始 ACP 事件。

---

## 6. 实施阶段

### P0 — 启动与回归验证

1. 在 `crates/luft-cli/src/backend.rs` 注册 `codex`。
2. 在 `config.rs` 新增 `backend.codex_acp`，并让 backend factory 只读取对应配置。
3. 支持 Windows `npx.cmd -y @agentclientprotocol/codex-acp` 与 Unix `npx -y @agentclientprotocol/codex-acp`。
4. 增加 `#[ignore]` 集成测试，执行简单 prompt，断言成功状态与非空文本。
5. `luft backend check codex` 只执行 ACP `initialize`，不创建 session，验证二进制与协议兼容性。

### P1 — 认证、审批与诊断

1. 开启并实现 ACP `authenticate`；认证方法由 adapter 的 initialize response 协商决定。
2. 实现安全的 `inherit_env` 与非敏感 `env` 注入。
3. 让 session mode、sandbox、approval、reasoning effort 通过 capability/config option 协商设置。
4. 保留 ACP stdout 专用于 JSON-RPC；采集受限长度的 stderr 尾部，在 spawn / handshake 失败时带入错误信息。
5. 为 Codex 增加 `APP_SERVER_LOGS` 诊断目录，并在文档中明确其位置。

### P2 — Codex 增强可观测性

1. 将 plan、review、subagent、web 与 image 事件从 `AcpRaw` 增量升格为结构化事件。
2. 在 web dashboard 显示 Codex subagent 的父线程关系与活动状态。
3. 为扩展 metadata 加版本兼容测试：字段缺失、未知字段、未知 tool type 都必须安全降级。
4. 评估 session/load 与会话复用；只有在可证明不会破坏 Luft 的隔离、重试、缓存与取消语义后再引入。

---

## 7. 验收标准

- `mock`、`opencode`、`loom-acp`、`codex` 可并存并被正确列出。
- Windows 与 Unix 都可通过 `npx` 启动 Codex ACP；全局 `codex-acp` 可替换而无代码改动。
- Codex 可完成文本 prompt、流式事件、权限请求、取消、超时与 structured-output MCP workflow。
- 无认证时返回清晰的认证错误；不会静默回退到 mock。
- API key 不写入仓库、全局 TOML、run 日志或 `AcpRaw`。
- 未识别的 Codex 扩展事件不会使 session 失败。
- OpenCode 与 Loom 现有测试保持通过。

---

## 8. 参考

- [ACP v1 protocol overview](https://agentclientprotocol.com/protocol/v1/overview)
- [`agentclientprotocol/codex-acp`](https://github.com/agentclientprotocol/codex-acp)
- [现有 ACP adapter 架构](../architecture/adapters.md)
- [后端管理 CLI 设计](./backend-command.md)
