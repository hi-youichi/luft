# commands 模块架构

> **CLI 子命令处理器（presentation 层）。** 每个 `.rs` 文件对应一个 clap 子命令，负责参数解析、用户交互（审批提示、进度输出）、输出格式化，核心逻辑委托给 `service`。

源码：[`src/commands/`](../../src/commands/) — 14 个子模块

---

## 1. 职责与边界

`commands` 是**表示层**：把 clap 解析出的参数转化为对 `service` 的调用，再把结果格式化为人类可读的输出。不包含任何 run 编排或持久化逻辑。

```
   main.rs (clap 解析 + dispatch)
       │
       ├── Commands::Run(args)     ──► commands::run::run_workflow(args)
       ├── Commands::Generate(args) ──► commands::generate::generate_script(args)
       ├── Commands::List { limit } ──► commands::list::list_runs_cmd(limit)
       ├── Commands::Status { ... }  ──► commands::status::status_run_cmd(...)
       ├── Commands::Logs { ... }    ──► commands::logs::logs_run_cmd(...)
       ├── Commands::Backend(cmd)    ──► commands::backend::{list,info,check,config,set}
       ├── Commands::Lua(cmd)        ──► commands::lua_validate::validate_lua(args)
       └── ...
```

**边界**：`commands` → `service` 单向依赖。`commands` 可以 import `service`、`core`（类型）、`backend`，但 `service` 不知道 `commands` 的存在。

---

## 2. 命令清单

| 子命令 | handler | 职责 |
|--------|---------|------|
| `run` | `run.rs::run_workflow` | 执行 workflow/NL，驱动输出（headless / phase renderer / event log） |
| `generate` | `generate.rs::generate_script` | NL → Lua 脚本（不执行），`-o` 写文件 |
| `list` | `list.rs::list_runs_cmd` | 列出历史 run + 状态 |
| `status` | `status.rs::status_run_cmd` | run 状态 + token + phase |
| `logs` | `logs.rs::logs_run_cmd` | 事件流日志 |
| `clear` | `clear.rs::clear_runs_cmd` | 清理 N 天前的已完成 run |
| `workflows` | `workflows.rs::list_workflows` | 列出 `~/.maestro/workflows/*.lua` |
| `save` | `save.rs::save_workflow` | 保存工作流（占位实现） |
| `backend` | `backend.rs::{list,info,check,config,set}` | 后端管理（5 个子命令） |
| `lua validate` | `lua_validate.rs::validate_lua` | Lua 脚本语法校验 |
| `mcp` | `mcp_server.rs::run` | MCP 结构化输出服务器 |

---

## 3. run 命令的输出三件套

`run_workflow` 是最复杂的 handler，内部使用三个 presentation 组件：

```
run_workflow(args)
│
├── PhaseRenderer   (phase_renderer.rs)
│     TUI 进度条：实时显示 phase / agent / log（stderr）
│
├── EventLogger     (event_log.rs)
│     可选文件日志：把事件流写入 --log <file>（pretty 或 jsonl）
│
└── ArtifactWriter  (artifact_writer.rs)
      最终报告写出：-o <file>，含 markdown 字段则写干净 Markdown
```

这三个组件都只处理**输出侧**，不参与 run 编排。

### 3.1 headless 模式

无 `--log` 时，run 走 headless 路径：事件以 JSONL 打到 stdout，最后输出 `{type:"report", ...}`。这是脚本/CI 友好的模式。

---

## 4. 设计决策

- **presentation / library 分层**：所有不涉及 I/O 的 run 逻辑在 `service`，`commands` 只管"怎么显示"。这让未来的 Web UI / TUI 可以复用 `service` 而不重写编排逻辑。
- **每个子命令一个文件**：避免单文件膨胀，新命令只需 `mod + handler + dispatch arm`。
- **`runs_base_dir()` 统一路径**：所有命令通过 `commands::runs_base_dir()` 获取 `./.maestro/runs`，不硬编码。
- **测试用 `GLOBAL_CWD_LOCK`**：部分命令测试需要切换工作目录，用模块级 `Mutex` 防止并行测试的 CWD 竞争。

---

## 5. 相关文档

- 总览：[../architecture.md](../architecture.md)
- 核心调用方：[cli.md](./cli.md)（`main.rs` dispatch → `commands`）
- 依赖的库层：[service.md](./service.md)
- 后端管理设计：[../design/backend-command.md](../design/backend-command.md)
