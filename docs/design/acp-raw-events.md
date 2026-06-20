# ACP 原始事件机制（`acp_raw`）— 实现设计

> **状态**: ✅ 已实现（2026-06-10）— 全 207 lib 测试 + 5 个新增测试通过
> **交叉参考**: [`p0-acp-backend.md`](./p0-acp-backend.md) — ACP 后端实现；[`websocket-server.md`](./websocket-server.md) — WS 订阅/事件流
> **相关代码**: [`src/adapters/update_mapper.rs`](../../src/adapters/update_mapper.rs)、[`src/core/contract/event.rs`](../../src/core/contract/event.rs)、[`src/ws/handler/subscription.rs`](../../src/ws/handler/subscription.rs)

---

## 0. 目标与定位

补全代码里已埋下的 `acp_raw` 伏笔（[`protocol.rs:104`](../../src/ws/protocol.rs#L104) 的订阅过滤注释："`None` means all projected events (excluding `acp_raw`)"，但实现缺失）。

把 ACP 的 `session/update` 通知**原样**作为一等 `AgentEvent` variant 流进现有 broadcast 总线。**生产端默认开启**——raw 始终进总线；但默认订阅与 journal 均不收 raw，需 WS 端显式 opt-in 订阅才可见。

**设计取向**（三个岔路）：

| 维度 | 取向 | 备选（留作后续） |
|---|---|---|
| ① 消费方 | WS 外部客户端（观测/调试） | Rust 内部 listener trait |
| ② 形态 | 原始透传（`serde_json::Value`） | 扩展归一化投影 / 两者都要 |
| ③ 范围 | 仅 `session/update` | + permission / 生命周期 / 全量 ACP 流量 |

---

## 1. 现状：ACP 事件如何流动

```
opencode acp (子进程)
        │  ACP JSON-RPC / stdio
        ▼
AcpAdapter (作为 ACP 客户端)
   ├─ on_receive_notification(SessionNotification)
   │      └─ update_mapper::handle_update(SessionUpdate)
   │            └─ 只把其中 4 种投影成 ProgressDelta → AgentEvent::AgentProgress
   ├─ on_receive_request(RequestPermissionRequest) → permission::decide
        ▼
   EventSender = broadcast::Sender<AgentEvent>   ← 现有唯一事件总线
        ▼
   ├─ WS 层: ServerMsg::Event { run_id, event }（带 subscribe 过滤）
   ├─ journal / 状态存储
   └─ TUI / headless JSONL
```

核心缺陷：[`update_mapper.rs:35-72`](../../src/adapters/update_mapper.rs#L35-L72) 的 `handle_update` 只匹配 `SessionUpdate` 的 4 种变体（`AgentMessageChunk` / `AgentThoughtChunk` / `ToolCall` / `ToolCallUpdate`），其余全部 `_ => {}` **丢弃**（Plan、AvailableCommandsUpdate、UserMessageChunk 等收不到），且没有任何"原始事件"出口。

---

## 2. 从代码得到的 4 个硬约束

决定本方案细节的现实约束：

| # | 约束 | 来源 | 影响 |
|---|------|------|------|
| 1 | 默认订阅 `passes_filter(None) => true` 会"全收" | [`subscription.rs:57-65`](../../src/ws/handler/subscription.rs#L57-L65) | 不改则默认订阅者被高频 raw 流刷爆 |
| 2 | journal forwarder 落盘**所有**总线事件，`get_logs` 又从中回读 | [`service/run.rs:217-227`](../../src/service/run.rs#L217-L227)、[`query.rs:103`](../../src/ws/handler/query.rs#L103) | raw 进总线会撑大 `events.jsonl` + 污染 `get_logs` |
| 3 | broadcast 总线有界（serve=256，其余=16） | [`scheduler/mod.rs:86`](../../src/core/scheduler/mod.rs#L86) | raw 高频，慢消费者 lag 丢帧 → 生产端需可关 |
| 4 | ACP 连接 `!Send`，跑在独立线程 `LocalSet` 内 | [`acp_adapter.rs:95-102`](../../src/adapters/acp_adapter.rs#L95-L102) | `events` 已 clone 进闭包，多传一个 `bool` 无线程安全负担 |

---

## 3. 改动清单（按文件）

P1 共触及 **7 个文件**（其中 2 处为必须补全的穷尽匹配）：

| # | 文件 | 修改内容 | 必要度 |
|---|------|---------|--------|
| ① | [`src/core/contract/event.rs`](../../src/core/contract/event.rs) | 给 `AgentEvent` 加 `AcpRaw { run_id, agent_id, kind, raw }` variant | 核心 |
| ② | [`src/adapters/update_mapper.rs`](../../src/adapters/update_mapper.rs) | `handle_update` 加 `emit_raw: bool` + 开头 emit `AcpRaw`；新增 `session_update_kind()`（覆盖全部变体） | 核心 |
| ③ | [`src/adapters/acp_adapter.rs`](../../src/adapters/acp_adapter.rs) | `AcpConfig.emit_raw_events: bool`（默认 `true`）；传进通知闭包 | 核心 |
| ④ | [`src/ws/protocol.rs`](../../src/ws/protocol.rs) | `event_type_name` 加 `AcpRaw => "acp_raw"`（**穷尽匹配·必须**）；`default_capabilities` 加 `"acp_raw"` | 必须 |
| ⑤ | [`src/ws/handler/subscription.rs`](../../src/ws/handler/subscription.rs) | `passes_filter` 的 `None` 分支排除 `acp_raw`；`event_run_id` 加 `AcpRaw`（**穷尽匹配·必须**） | 必须 |
| ⑥ | [`src/service/run.rs`](../../src/service/run.rs#L217-L227) | journal forwarder 跳过 `AcpRaw`，不落盘 | 核心 |
| ⑦ | [`src/backend.rs`](../../src/backend.rs#L16-L19) | 把 `--no-acp-raw` opt-out 标志接到 `AcpConfig`（P2 再接 `serve` 侧） | 接线 |

**编译与测试补充：**

- **必然报错（编译器兜底）**：④⑤ 两处穷尽匹配（`event_type_name`、`event_run_id`）——加 variant 后想漏都漏不掉。
- **无需改动**（带 catch-all `_`）：[`src/core/state.rs`](../../src/core/state.rs)、[`src/ws/handler/query.rs`](../../src/ws/handler/query.rs)。
- **测试要同步**：②③⑤ 现有测试中调用 `handle_update(...)` 处补 `emit_raw` 参数；④ 的 `default_capabilities` 的 `len()` 断言更新。
- **默认开启下最关键**：⑤（`passes_filter` 的 `None` 排除）和 ⑥（journal 跳过）——这两处失效会导致 raw 常态灌满所有客户端与 `events.jsonl`。

各文件详细改动如下。

### ① 事件类型 — [`src/core/contract/event.rs`](../../src/core/contract/event.rs)

`AgentEvent` 增加 variant（snake_case → serde tag 自动为 `acp_raw`）：

```rust
/// 原始 ACP session/update 透传（仅 emit_raw 开启时产生）。
AcpRaw {
    run_id: RunId,
    agent_id: AgentId,
    /// SessionUpdate 变体判别子，如 "agent_message_chunk" / "plan"，
    /// 让下游无需解析 raw 即可过滤。
    kind: String,
    /// 原样序列化的 ACP SessionUpdate。
    raw: serde_json::Value,
},
```

> **取舍**：`raw` 用 `serde_json::Value` 而非具体 ACP 类型，沿用 update_mapper "不依赖宏生成嵌套类型" 的既有风格，保证忠实度且不把 ACP schema 泄漏进 contract 层。

### ② 映射层 — [`src/adapters/update_mapper.rs`](../../src/adapters/update_mapper.rs)

`handle_update` 增参 `emit_raw: bool`，函数开头先发 raw（投影逻辑完全不动，原有 4 种照常走 `AgentProgress`）：

```rust
pub fn handle_update(
    update: &SessionUpdate,
    run_id: RunId,
    agent_id: AgentId,
    acc: &Accumulator,
    events: &EventSender,
    emit_raw: bool,
) {
    if emit_raw {
        let _ = events.send(AgentEvent::AcpRaw {
            run_id,
            agent_id,
            kind: session_update_kind(update).to_string(),
            raw: to_json(update),
        });
    }
    match update { /* 原样不变 */ }
    // usage 累积原样不变
}
```

`kind` 直接从序列化后的 `sessionUpdate` 标签字段读取（`SessionUpdate` 是 `#[serde(tag = "sessionUpdate", rename_all = "snake_case")]`）：

```rust
let raw = to_json(update);
let kind = raw.get("sessionUpdate").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
```

> **为何不写 `match` 提取 kind**：`SessionUpdate` 在 schema crate 里标了 `#[non_exhaustive]`，外部无法穷尽匹配；从 JSON 标签读取既稳健、又能**覆盖全部变体**（含现在 `_ => {}` 丢掉的 Plan / AvailableCommandsUpdate / UserMessageChunk 等）——这正是"能收到更多事件"的关键。

### ③ 适配器 — [`src/adapters/acp_adapter.rs`](../../src/adapters/acp_adapter.rs)

- `AcpConfig` 增 `pub emit_raw_events: bool`（**`Default` 为 `true`**，默认开启）。
- `run_acp_session` 把 `config.emit_raw_events` 捕获进 `on_receive_notification` 闭包，传给 `handle_update(..., emit_raw)`（[`acp_adapter.rs:158-170`](../../src/adapters/acp_adapter.rs#L158-L170)）。

### ④ WS 协议 — [`src/ws/protocol.rs`](../../src/ws/protocol.rs)

- `event_type_name` 加一臂：`AgentEvent::AcpRaw { .. } => "acp_raw"`（**穷尽匹配，必须加**）。
- `default_capabilities` 追加 `"acp_raw"`，让客户端能发现该能力（同步更新对应断言的 `len()`）。

### ⑤ WS 过滤 — [`src/ws/handler/subscription.rs`](../../src/ws/handler/subscription.rs)

`passes_filter` 的 `None` 分支兑现注释承诺：

```rust
None => event_type_name(evt) != "acp_raw",   // 默认排除高频 raw
Some(types) => {                              // 显式 opt-in 不变
    let name = event_type_name(evt);
    types.iter().any(|t| t == name)
}
```

`event_run_id` 加 `AgentEvent::AcpRaw { run_id, .. } => *run_id`（**穷尽匹配，必须加**）。

### ⑥ journal 不落盘 raw — [`src/service/run.rs`](../../src/service/run.rs#L217-L227)

forwarder 跳过 raw，保持 `events.jsonl` / `get_logs` 干净：

```rust
Ok(evt) => {
    if !matches!(evt, AgentEvent::AcpRaw { .. }) {
        let _ = store.append_event(&evt);
    }
}
```

> **理由**：raw 是"实时观测流"，不是持久历史。若以后要持久化，单独写 `acp_raw.jsonl`（见 §7）。

### ⑦ 接通开关 — [`src/backend.rs:16-19`](../../src/backend.rs#L16-L19)

`create_backend(id, emit_raw_events)` 把开关透传进 `AcpConfig`（mock 后端忽略该参数）。`serve` 与 `run` 两个命令都提供 **`--no-acp-raw` 退出开关**（opt-out）：默认开启，加标志即构造 `emit_raw_events: false` 的 `AcpConfig`，用于高负载/内存敏感场景或避免 headless JSONL 被 raw 刷屏。这是**进程级全局开关**（非 per-subscription），是本阶段刻意的简化，限制与升级路径见 §8。

---

## 4. 端到端数据流（开启后）

```
opencode acp ──session/update──▶ on_receive_notification
   └─ handle_update(emit_raw=true)
        ├─ AgentEvent::AcpRaw{kind,raw}  ─┐
        └─ AgentProgress (原投影)         ─┤
                                          ▼ broadcast 总线
                       ┌──────────────────┼───────────────────┐
              WS filter=["acp_raw"]   WS filter=None        journal forwarder
                   收 raw             不收 raw(§2.1)        跳过 raw(§2.6)
```

WS 客户端用法：

```jsonc
// 订阅时显式 opt-in raw（可与其他类型混合）
{ "type": "subscribe", "id": "1",
  "payload": { "run_id": "...", "filter": ["acp_raw", "agent_done"] } }

// 收到的事件
{ "type": "event", "run_id": "...",
  "event": { "type": "acp_raw", "agent_id": "...",
             "kind": "plan", "raw": { /* 原始 SessionUpdate */ } } }
```

---

## 5. 默认行为与开销

- `emit_raw_events` **默认 `true`** → raw 始终进总线。
- **默认对外不可见**：默认订阅（`filter:None`）排除 raw（§2 约束 1），journal forwarder 跳过 raw（§3.⑥）。只有显式 `filter:["acp_raw"]` 的订阅者才收到。
- **始终付出的开销**（默认开启的代价）：每条 `SessionUpdate` 都会序列化为 JSON 并发进有界 broadcast 总线；总线把它投递给**每个** receiver（journal forwarder + 每个 WS 订阅）后再各自丢弃。高频 raw 会加快环形缓冲区翻滚，慢消费者更易 lag 丢帧（§2 约束 3）。内存/高负载敏感场景用 `--no-acp-raw` 关闭（§3.⑦）。

---

## 6. 测试计划

贴合现有 `#[cfg(test)]` 风格：

- **update_mapper**：`emit_raw=true` 时 `AgentMessageChunk` 额外收到一条 `AcpRaw`（`kind=="agent_message_chunk"`，`raw` 含原文）；`emit_raw=false` 不多发；对原本 `_ => {}` 丢弃的变体（如 `Plan`）现在能产出 `AcpRaw`。现有调用 `handle_update` 处统一补 `false` 参数。
- **protocol**：`event_type_name(AcpRaw) == "acp_raw"`；`default_capabilities` 含 `"acp_raw"` 并更新 `len` 断言。
- **subscription**：`passes_filter(AcpRaw, None) == false`；`passes_filter(AcpRaw, Some(["acp_raw"])) == true`；`event_run_id` 覆盖 `AcpRaw`。
- **service/run**：forwarder 收到 `AcpRaw` 不调用 `append_event`。

---

## 7. 风险 / 兼容性

- **穷尽匹配**仅 2 处需改（`event_type_name`、`event_run_id`）；[`state.rs`](../../src/core/state.rs)、[`query.rs`](../../src/ws/handler/query.rs) 都带 `_` catch-all，不受影响——加完 `cargo build` 由编译器兜底验证。
- **协议向后兼容**：新增 variant + 新 capability，老客户端不订阅则完全无感。
- **流量**：开标志且有订阅者时，慢消费者可能 lag 丢帧（broadcast 语义），属可接受的调试取舍。

---

## 8. 分阶段与后续升级路径

| 阶段 | 内容 | 状态 |
|---|---|---|
| **P1（本方案）** | §3 全部改动，默认开启；`serve`/`run --no-acp-raw` 退出开关接线。补全 `acp_raw`。 | ✅ 已完成 |
| **P2** | 协议文档/客户端示例补充（§4 已含 JSON 用例）。 | 待办 |
| **P3（按需）** | a) per-run / per-subscription 开关（穿 `RunContext` 替代全局 flag）；b) 单独 `acp_raw.jsonl` 持久化 + 回放；c) 扩展归一化投影（②）或纳入 permission / 全量流量（③）。 | 待办 |
