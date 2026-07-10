# `luft backend` 命令 — 后端管理 CLI

> **路线图引用**: `roadmap.md` §CLI 增强
> **状态**: 设计阶段
> **交叉参考**: `cli.md` — 当前 CLI 架构；`adapters.md` — 后端适配器架构；`p0-acp-backend.md` — ACP 后端实现设计

---

## 1. 现状问题

当前 CLI 对后端的管理能力为零：

| # | 问题 | 影响 |
|---|------|------|
| 1 | 无命令查看可用后端列表 | 用户不知道有 `mock`/`opencode` 两种选择 |
| 2 | 无后端连通性检测 | `luft run` 在 opencode 未安装时静默 fallback 到 mock，或报出不易理解的 spawn 错误 |
| 3 | 后端选择仅靠 `--backend` 参数 + auto-detect | 无法持久化默认后端偏好 |
| 4 | `AcpConfig` 字段（binary, log-level, timeout）不可在外部查看或修改 | 调试需改代码 |
| 5 | 在 `opencode` 未安装时，`luft run` NL 模式的报错信息不精确 | 用户付出时间等待 planner←→mock 循环耗尽后才知道问题 |

用户痛点场景：

```
$ luft run "write a research report"
# ... 等 30+ 秒，mock backend 反复重试 planner 后 ...
Error: planner exhausted
# 实际问题是：opencode 未安装，auto-detect 返回了 mock
```

---

## 2. 目标架构

新增 `luft backend` 子命令，5 个子子命令：

```
luft backend
├── list      列出所有可用后端 (id + 能力)
├── info [id] 查看指定后端的详细配置与能力
├── check [id]探测后端是否可用（binary 在 PATH？能否握手？）
├── config    查看/修改后端配置（binary, log-level, timeout）
└── set <id>  设置默认后端（持久化到 config 文件）
```

### 2.1 模块布局

```
src/
├── commands/
│   ├── mod.rs              # 添加 pub mod backend;
│   └── backend.rs          # 新命令 handler
├── config.rs               # (新增) config 文件读写
└── main.rs                 # 添加 Commands::Backend variant
```

### 2.2 config.rs — 持久化配置

```toml
# $XDG_CONFIG_HOME/luft/config.toml
[backend]
default = "opencode"

[backend.acp]
binary = "opencode"              # 或 /absolute/path/to/opencode
log_level = "debug"              # 传给 --log-level
connect_timeout_secs = 10        # initialize 握手超时
idle_timeout_secs = 300          # 会话空闲超时（无协议消息时 kill）
emit_raw_events = true           # 透传 ACP session/update 为 acp_raw 事件
```

设计要点：

- 配置路径：`dirs::config_dir()/luft/config.toml`（跨平台：Linux `~/.config/`, macOS `~/Library/Application Support/`, Windows `%APPDATA%`）
- 优先级链：CLI 参数 `--backend` > config 文件 `default` > auto-detect
- info / check 不指定 id 时使用默认后端（优先级链解析结果）
- 只读操作（list/info/check）不依赖 config 文件；写操作（config/set）在无 config 文件时自动创建父目录及文件

`config.rs` 公开 API：

```rust
pub struct LuftConfig {
    pub backend: BackendConfig,
}

pub struct BackendConfig {
    pub default: Option<String>,
    pub acp: AcpConfigOverride,
}

pub struct AcpConfigOverride {
    pub binary: Option<PathBuf>,
    pub log_level: Option<String>,
    /// Serde: 存为 u64 秒数，反序列化时转为 Duration::from_secs
    pub connect_timeout_secs: Option<u64>,
    pub idle_timeout_secs: Option<u64>,
    pub emit_raw_events: Option<bool>,
}

pub fn load_config() -> Result<Option<LuftConfig>>;
pub fn save_config(config: &LuftConfig) -> Result<()>;
pub fn merge_with_backend_factory(user_backend: Option<&str>) -> String;  // 参数 > config > detect
```

### 2.3 backend.rs — 命令 handler

```rust
pub enum BackendSubcommand {
    /// 列出所有可用后端 (id + 能力).
    List,
    /// 查看指定后端的详细配置与能力.
    Info { id: Option<String> },
    /// 探测后端是否可用.
    Check { id: Option<String> },
    /// 查看/修改后端配置.
    Config { key: Option<String>, value: Option<String> },
    /// 设置默认后端.
    Set { id: String },
}
```

每个子命令的 handler 函数：

| 函数 | 实现方式 |
|------|---------|
| `list_backends()` | 硬编码已知后端列表（`mock`, `opencode`），各自调用 `capabilities()` |
| `info_backend(id)` | 通过优先级链解析 id → `backend::create_backend` → 打印全量能力 + 配置 |
| `check_backend(id)` | `which_exists` 检测 binary；opencode 时尝试 `--version` 确认是 ACP agent；mock 写死 `Ok` |
| `config_backend(key, val)` | 无参数 → 以 JSON 打印 `LuftConfig`；有 `key=value` → 更新并落盘 |
| `set_default_backend(id)` | 更新 `LuftConfig.backend.default` 并落盘；不做 id 校验（run 时失败） |

---

## 3. 用户交互示例

### 3.1 `luft backend list`

```
$ luft backend list
     id     │ streaming │ mcp_injection │ structured_output │ models
────────────┼───────────┼───────────────┼───────────────────┼─────────
  opencode  │       ✓   │           ✓   │                ✓  │ (any)
  mock      │       ✗   │           ✗   │                ✗  │ (n/a)
```

### 3.2 `luft backend check opencode`

```
$ luft backend check opencode
✓ opencode binary found at /usr/local/bin/opencode (v1.2.3)
✓ ACP initialize handshake succeeded (42ms)

$ luft backend check mock
✓ mock backend is always available
```

### 3.3 `luft backend info`

```
$ luft backend info opencode
{
  "id": "opencode",
  "capabilities": {
    "streaming": true,
    "mcp_injection": true,
    "structured_output": true,
    "models": []
  },
  "config": {
    "binary": "opencode",
    "log_level": null,
    "connect_timeout_secs": 10,
    "idle_timeout_secs": 300,
    "emit_raw_events": true
  }
}

$ luft backend info
# 未指定 id → 使用默认后端 (通过优先级链解析)
{
  "id": "opencode",
  ...
}
```

### 3.4 `luft backend config`

```
$ luft backend config
{
    "backend": {
      "default": "opencode",
      "acp": {
        "binary": "opencode",
        "log_level": null,
        "connect_timeout_secs": 10,
        "idle_timeout_secs": 300,
        "emit_raw_events": true
      }
    }
}

$ luft backend config backend.acp.log_level debug
✓ config updated
```

### 3.5 `luft backend set mock`

```
$ luft backend set mock
✓ Config saved: default backend = "mock"
```

---

## 4. 关键设计决策

| # | 决策 | 选择 | 理由 |
|---|------|------|------|
| D1 | 配置格式 | TOML（`toml` crate） | Rust 生态标准，可读性好，与 Cargo.toml 同风格 |
| D2 | 配置路径 | `dirs::config_dir()/luft/config.toml` | XDG 标准（跨平台），非项目本地 |
| D3 | 优先级链 | CLI 参数 > config 文件 > auto-detect | 最可预测：显式 > 持久化 > 自动 |
| D4 | `check` 是否真连 ACP | 发 initialize 但不发 session/new | 验证 binary 是 ACP agent 而非壳，但避免副作用 |
| D5 | mock 的 `check` 行为 | 永远返回可用 | MockBackend 无需外部依赖 |
| D6 | 输出格式 | 手写 ASCII 表格（list）/ JSON（info/config） | list 零额外依赖；info/config 可脚本化 |
| D7 | Duration 序列化 | TOML 存 u64 秒数，加载时 `Duration::from_secs` | TOML 无原生 Duration 类型，秒级精度足够 |

---

## 5. config 优先级链（设计细节）

```
         CLI 参数                Config 文件               Auto-detect
    luft run --backend X    $XDG_CONFIG_HOME/         detect_available_backends()
                                luft/config.toml       (扫描 opencode/loom-acp)
        ↓                         ↓                         ↓
    explicit                   persisted                  fallback
    (最高优先级)                 (次高)                    (最低)

    resolve_default_backend:
       用户指定 → 用指定值
       无指定 → config.default 有 → 用 config 值
       无指定 → config.default 无 → detect_available_backends():
           0 个真实后端 → "mock"（静默回退）
           1 个        → 直接使用
           ≥2 个        → prompt_backend_selection() 交互选择，结果持久化到 config
```

非交互环境（管道/CI，`console::user_attended() == false`）跳过提示，自动选第一个可用后端。

---

## 6. 当前状态与局限（v0.1）

- v0.1 内置 `mock`、`opencode`、`loom-acp` 三个后端；多后端插件化是 v0.2
- `check` 的 ACP 握手不会真正发 `session/new`+`session/prompt`，因此无法验证 LLM 本身的可用性（API key、额度等）
- `config` 命令的 key 路径当前只支持后端相关字段（`backend.*`）；全局配置扩展留待后续
- `set` 命令不做后端 id 校验（允许设一个不存在的 id），`run` 时失败

---

## 7. 实现路线

### Phase 1 — 只读命令（不含 config 依赖）

| 任务 | 文件 | 依赖 |
|------|------|------|
| `backend list` 子命令 | `commands/backend.rs`, `main.rs` | 手写 ASCII 表格 |
| `backend info` 子命令 | `commands/backend.rs` | `AcpConfig::default()` 实例化 |
| `backend check` 子命令 | `commands/backend.rs`, `backend.rs` | 现有 `which_exists` |

### Phase 2 — 持久化命令（含 config 文件）

| 任务 | 文件 | 依赖 |
|------|------|------|
| `config.rs` 读写 | `config.rs` | `toml` + `dirs` crate |
| `backend config` 子命令 | `commands/backend.rs` | `config.rs` |
| `backend set` 子命令 | `commands/backend.rs` | `config.rs` |
| 修改 `create_backend` 优先级链 | `backend.rs` | `config.rs` |

### Phase 3 — 整合测试

| 任务 | 类型 |
|------|------|
| `backend list` 列出已知后端 | 单测 |
| `backend check mock` 返回可用 | 单测 |
| `backend check opencode` 在有/无 binary 时 | 集成测试（`#[ignore]`） |
| config 读写 round-trip | 单测（tempfile） |
| 优先级链：参数 > config > detect | 单测 |

---

## 8. 新增依赖

```toml
# Phase 1（无新增依赖，复用已有）
# Phase 2
toml = "0.8"        # config 序列化/反序列化
dirs = "6"          # XDG config 目录（跨平台）
```

---

## 9. 相关文档

- 总览：[../architecture.md](../architecture.md)
- CLI 架构：[../architecture/cli.md](../architecture/cli.md)
- 后端适配器：[../architecture/adapters.md](../architecture/adapters.md)
- 后端工厂：`../../src/backend.rs`
- ACP 后端设计：[./p0-acp-backend.md](./p0-acp-backend.md)
