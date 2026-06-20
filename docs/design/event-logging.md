# 日志体系：程序日志 + 事件日志 — 实现设计

> **状态**: 🚧 P2 已实现（2026-06-11）— `EventLogger` + 穷尽 `format_event_line` + `run --log/--log-format`（pretty/jsonl），手验通过。程序日志（P1）见 [`program-logging.md`](./program-logging.md)；P3 `listen` 待办。
> **交叉参考**: [`program-logging.md`](./program-logging.md) — 另一平面（程序日志 tracing，§4 的完整展开）；[`acp-raw-events.md`](./acp-raw-events.md)、[`sdk-events.md`](./sdk-events.md) — 事件来源；[`websocket-server.md`](./websocket-server.md) — WS 订阅协议
> **相关代码**: [`src/commands/run.rs`](../../src/commands/run.rs)、[`src/ws/protocol.rs`](../../src/ws/protocol.rs)、[`src/core/contract/event.rs`](../../src/core/contract/event.rs)、[`src/adapters/acp_adapter.rs`](../../src/adapters/acp_adapter.rs)

---

## 0. 目标：两个日志平面

两类**互补**的日志：

- **程序日志（program log）** — 运维/诊断：spawn 失败、协议错误、连接、重试、debug 跟踪。机制 `tracing` + `tracing_subscriber`，写 **stderr**（+可选文件）。
- **事件日志（event log）** — 领域/工作流：`agent`/`phase`/`sdk_*`/`acp_raw` 等事件。机制 [`AgentEvent`](../../src/core/contract/event.rs) 总线 + `EventLogger`。

**event log 是 program log 的补充**：前者答"工作流做了什么"，后者答"程序怎么运行的"。

---

## 1. 现状（两平面都不完整）

| 平面 | 现状 |
|---|---|
| 程序日志 | **几乎空白**：有 `tracing = "0.1"` 但**无 subscriber**；且 `tracing::*` 调用**只有 3 处、全在 ws 层**。`runtime`/`scheduler`/`adapters`/`sdk`/`service` **零 tracing** → `run` 这条路径即使装了 subscriber 也**收不到任何东西**。诊断目前靠零散 `eprintln!`。 |
| 事件日志 | 总线 + 每-run `events.jsonl`（[`service/run.rs:221`](../../src/service/run.rs#L221)，**注意：跳过 `acp_raw`**）+ headless JSONL（[`drain_events_jsonl`](../../src/commands/run.rs#L160)）已就绪；缺统一 `EventLogger` 与远程 `listen`。 |

> **评审结论**：程序日志真正的工作量是**埋点**（§4.1），不是装 subscriber；装 subscriber 只是让埋点可见。

---

## 2. 设计决策（已锁定）

**程序日志**

| 维度 | 决定 |
|---|---|
| 机制 | `tracing` + `tracing_subscriber`（`env-filter` + `fmt`） |
| **埋点** | 给关键诊断点补 `tracing::{warn,error,info,debug}`（§4.1 清单）——核心工作 |
| sink | 默认 **stderr**；`serve --log-file <path>` 可加文件 |
| 级别 | `--log-level` > `RUST_LOG` > 缺省（`serve`/`listen`=`info`、`run`=`warn`） |
| 格式 | 文本（默认）；JSON 留作后续 |

**事件日志**

| 维度 | 决定 |
|---|---|
| 组件 | **来源无关**的 `EventLogger`，只吃 `AgentEvent` |
| 默认格式 | **`pretty`（人读）**；`jsonl` 可选 |
| `format_event_line` | **穷尽 `match`（禁 `_`）**，新增事件变体由编译器强制覆盖 |
| `run` 日志 | **显式 `--log <path>` 才写**，默认不写 |
| 远程监听 | 新增 `listen` WS 客户端子命令 |

---

## 3. 双平面边界规则 + stderr 共存

- **程序内部诊断 → 只进 tracing**（不要变成 `AgentEvent`）。
- **脚本 `log("…")` 和领域事件 → 只进 event 平面**（`AgentEvent::Log` 等）。
- **stdout 留给数据**（headless 的 JSONL / 最终 report）；**程序日志一律 stderr**。
- **stderr 共存**：TUI 的 [`print_progress`](../../src/commands/run.rs#L129) 也走 stderr。解决：`run` 程序日志缺省级别 `warn`（§2），平时近乎静默，只在真出问题时与进度交织；二者职责不同（进度=事件摘要，程序日志=诊断），可接受。headless 模式无 stderr 进度（事件走 stdout），stderr 仅程序日志。
- 可选（默认关）：tracing 的 `ERROR` 桥接成 `AgentEvent::Log{level:error}`。

---

## 4. 程序日志（tracing）→ 见 [`program-logging.md`](./program-logging.md)

程序日志（运维/诊断平面）的完整方案已独立成文：[`program-logging.md`](./program-logging.md) —— span 层级、级别/字段约定、**埋点地图**、`logging.rs` 初始化、验收标准、分阶段。

本文只需记住与事件日志相关的两点：
- **stdout 留数据、程序日志走 stderr**（§3）；`run` 缺省 `warn` 故近乎静默。
- **`run_id` 是两平面的连接键**：程序日志每行带 `run_id`，与本文的 `events.jsonl` 同键可对照。

---

## 5. 事件日志：`EventLogger`（program log 的补充）

新文件 [`src/commands/event_log.rs`](../../src/commands/event_log.rs)：

```rust
pub enum LogFormat { Pretty, Jsonl }     // 默认 Pretty

pub struct EventLogger {
    sink: std::io::BufWriter<Box<dyn std::io::Write + Send>>,  // 缓冲，避免高频 acp_raw 每行 flush
    format: LogFormat,
}

impl EventLogger {
    pub fn new(out: Option<&std::path::Path>, format: LogFormat) -> anyhow::Result<Self>;
    /// 写一行；周期/空闲 flush（见下）。
    pub fn write(&mut self, evt: &AgentEvent) -> anyhow::Result<()>;
    pub fn flush(&mut self) -> anyhow::Result<()>;
}

/// pretty 单行格式化，**穷尽 match 覆盖全部事件类型**（禁 `_`，新增变体编译器强制更新）。
pub fn format_event_line(evt: &AgentEvent) -> String;
```

- `Jsonl` → `serde_json::to_string(evt)?`；`Pretty` → `format_event_line(evt)`。
- **flush 策略**：`BufWriter` + 每收一批/空闲时 flush（不每行 flush）；`run_done`/退出前强制 flush。高频 `acp_raw` 下避免 syscall 风暴。
- 与 [`print_progress`](../../src/commands/run.rs#L129) 共用 `format_event_line`，摘要风格一致。

---

## 6. 一致性不变量（已收窄）

**保证**：对**同一个 `AgentEvent`**，`EventLogger(Jsonl)` 写出的行 == `serde_json::to_string(evt)` == `events.jsonl` 里该事件的行（**单条序列化字节相等**）。

**不保证**：不同 sink 的**事件流/集合相同**。原因：
- `events.jsonl` **不含 `acp_raw`**（forwarder 故意跳过）；
- `run --log`（本地总线，`acp_raw` 默认开）**含** `acp_raw`，而 `listen` 缺省 filter **排除** `acp_raw` → 默认就分叉；
- broadcast 有界，慢消费者 lag 丢帧 → 各 sink 完整性/顺序可能不同。

**要让两端真正对齐**，需同时满足：① 相同 `filter` 集合；② 相同来源覆盖（如都含/都不含 `acp_raw`）；③ 缓冲足够不丢帧。文档/CLI 不隐式承诺"流相同"，只承诺"同一事件序列化相同"。

---

## 7. 两端 wiring（事件日志，同一个 logger）

| 端 | 来源 | 接法 |
|---|---|---|
| 本地 `run` | 本地 broadcast 总线（已是 `AgentEvent`） | drain 的 emit 闭包喂 `logger.write(evt)` |
| 远程 `listen` | WS 帧 → `.event` 反序列化为 `AgentEvent` | 同一个 `logger.write(evt)` |

### 7.1 `run`
```
maestro run … --log <path> [--log-format pretty|jsonl]
```
缺省不写文件；给定 `--log` 时 drain 中附带 `EventLogger`（默认 pretty）；`print_progress` 复用 `format_event_line`。

### 7.2 `listen`（新）
```
maestro listen <RUN_ID>
  --url ws://127.0.0.1:8080/ws  --out <path>  --filter acp_raw,sdk_events,…
  --format pretty|jsonl         --follow
```
连接（`tokio-tungstenite`；`wss` 需 TLS feature，留作后续）→ 发手搓 `subscribe` JSON → **收帧分派**：

| 帧 `type` | 处理 |
|---|---|
| `hello` | 校验/忽略 |
| `ok` | 订阅成功，进入流 |
| `event` | 取 `.event` 反序列化 `AgentEvent` → `logger.write`；若是 `run_done` 且无 `--follow` → flush 退出 |
| `error`（`not_found`/`run_finished`） | 走程序日志（stderr）报错；**run 已结束无实时流** → 当前直接非零退出（回退读 `events.jsonl` 留作后续） |
| `server_closing` | flush + 退出 |

Ctrl-C → flush + 退出。**解耦**：只手搓 subscribe + 反序列化 `event` 字段，不动 `protocol.rs` 的 derive。

---

## 8. 改动文件清单（事件日志部分）

> 程序日志的改动（`logging.rs`、埋点、`serve --log-file`、`Cargo.toml` 的 `tracing-subscriber`）见 [`program-logging.md`](./program-logging.md) §5/§10。

| 文件 | 改动 |
|---|---|
| `Cargo.toml` | 加 `tokio-tungstenite` |
| [`src/commands/event_log.rs`](../../src/commands/event_log.rs)（新） | `EventLogger` + `format_event_line`（穷尽） |
| [`src/commands/listen.rs`](../../src/commands/listen.rs)（新） | WS 客户端 → `EventLogger`，全帧分派 |
| [`src/commands/run.rs`](../../src/commands/run.rs) | `--log`/`--log-format`；`print_progress` 复用 formatter |
| [`src/main.rs`](../../src/main.rs) | 注册 `Listen`；`run` 加 `--log`（`--log-level`/`logging::init` 见 program-logging.md） |

---

## 9. 分阶段

整体三平面（程序日志 / 事件日志 / 远程监听）耦合度低，可独立交付：

| 阶段 | 内容 | 价值 |
|---|---|---|
| **P1 程序日志** | 见 [`program-logging.md`](./program-logging.md) §10（P1a–P1d） | ✅ 已完成 |
| **P2 事件日志核心** | `event_log.rs`（`EventLogger` + 穷尽 `format_event_line`）+ `run --log` | ✅ 已完成 |
| **P3 远程监听** | `listen` 子命令 + `tokio-tungstenite` + 全帧分派 | 待办 |

---

## 10. 测试（事件日志部分）

- **`format_event_line` 单测**：每种事件类型 → 期望摘要行（穷尽，编译器兜底）。
- **一致性 round-trip（关键）**：构造 `AgentEvent` → 直接喂 `EventLogger`，与"序列化成 `ServerMsg::Event` JSON 再解回 `AgentEvent`"喂同一 logger，断言**两行字节相同**（模拟 WS 往返，不起真 socket）。
- **jsonl 对齐**：`EventLogger(Jsonl)` 行 == `serde_json::to_string(evt)`。
- **`listen` 帧分派单测**：喂 `hello`/`ok`/`event`/`error`/`server_closing` JSON，断言各自行为（event→写、error→退、closing→退）。
- 程序日志侧测试见 [`program-logging.md`](./program-logging.md) §11。

---

## 11. 留待后续

- 程序日志 JSON 格式 / 文件轮转；`ERROR → AgentEvent::Log` 桥接（默认关）。
- `listen` 对"run 已结束"回退读 `events.jsonl`；断线重连；`wss`/TLS。
- `format_event_line` 彩色/对齐美化。
