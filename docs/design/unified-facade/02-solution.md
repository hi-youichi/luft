# 2. 方案：修复依赖倒置 + 统一 Service 层

## 2.1 根因分析

```
当前依赖方向（倒置）:
  luft ──> luft-service ──> luft-core / luft-runtime / luft-storage / luft-planner
                        ↑
                        └── 这些模块本应在底层：
                            query.rs      (只依赖 luft-core + chrono)
                            params.rs     (只依赖 serde_json + json_to_lua)
                            phases.rs     (只依赖 luft-core + luft-planner)
                            json_to_lua.rs (只依赖 serde_json)
                            run.rs        (依赖 luft-core + luft-runtime + luft-storage + luft-planner)
```

`luft` 的 `builder.rs` 调用 `luft_service::query::*` 和 `luft_service::run::*` 来执行引擎操作 — 这些函数本质上是引擎层的实现，却被放在了名为 "service" 的上层 crate 里。

## 2.2 修复方案：模块下沉

| 模块 | 当前位置 | 目标位置 | 依赖 | 难度 |
|------|---------|---------|------|------|
| `json_to_lua.rs` | `luft-service` | `luft-core` | 纯 `serde_json` | 简单 |
| `params.rs` | `luft-service` | `luft-core` | `serde_json` + `json_to_lua` | 简单 |
| `query.rs` | `luft-service` | `luft-core` | `luft-core` 类型 + `chrono` | 简单 |
| `phases.rs` | `luft-service` | `luft-core` | `luft-core` + `luft-planner` | 中等 |
| `run.rs` | `luft-service` | `luft`（合并） | `luft-runtime` + `luft-storage` + `luft-planner` | 中等 |

下沉后 `luft` 不再依赖 `luft-service`，依赖方向恢复正常：

```
修复后依赖方向:
  luft-service ──> luft ──> luft-core (含 query/params/phases/json_to_lua)
                │          ├──> luft-runtime
                │          ├──> luft-storage
                │          └──> luft-planner
                ├──> luft-core
                └──> rmcp (schemars re-export for Request types)

  luft-cli  ──> luft-service ──> luft
  luft-mcp  ──> luft-service ──> luft
  loom      ──> luft-service ──> luft
```

`luft-service` 现在可以正常依赖 `luft`，`WorkflowServiceImpl`（需要 `Luft`）可以归入 `luft-service`。`start_workflow` 返回的 `RunHandle`（`luft` 类型）也可以放进 trait。
