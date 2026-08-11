--------------------------------------------
-- Goal: Produce complete source-grounded documentation for Luft
-- Arch:
--   discover ==> inspect in parallel ==> draft ==> verify ==> report
-- Flow: repository map -> evidence[] -> complete document -> verified report
--------------------------------------------
-- 基于源码证据编写完整技术文档。
--
-- 参数：
--   args.document_path 目标文档路径，默认 docs/design/complete-documentation.md
--   args.scope          文档范围，默认整个 Luft 仓库

meta = {
    phases = {
        { label = "discover", description = "盘点仓库结构、入口、模块和现有文档", agents = 1 },
        { label = "inspect", description = "并行分析架构、运行链路、配置/API 和测试", agents = 4 },
        { label = "draft", description = "根据源码证据写入完整技术文档", agents = 1 },
        { label = "verify", description = "逐项核验文档引用和技术结论", agents = 1 },
    },
    reasoning = "先建立源码地图，再从多个角度收集证据，由单一作者统一成文，最后回读源码校验路径、符号和行为描述。",
}

local function text_of(result)
    if result and result.ok then
        return tostring(result.output or "")
    end
    return "[agent failed: " .. tostring(result and result.status or "unknown") .. "]"
end

function main()
    budget(360000, 40)

    local document_path = (args and args.document_path) or "docs/design/complete-documentation.md"
    local scope = (args and args.scope) or "整个 Luft 仓库"

    phase("discover", 1)
    local discovery = agent({
        name = "documentation-discover",
        description = "建立 Luft 仓库源码地图",
        prompt = "你是 Luft 项目的源码勘探员。只读检查 C:\\Users\\heycj\\dev\\luft，范围是：" .. scope .. "。"
            .. "使用 rg、git status、git diff --stat、Get-Content 等工具，盘点 workspace/crates、CLI 入口、公共 facade、runtime/scheduler、storage、service、MCP/ACP、配置、测试、示例和现有文档。"
            .. "必须区分当前工作树未提交修改与基线代码。输出普通文本，包含真实相对路径、关键符号、行号、主要执行链路、外部入口、数据流和文档缺口；不要修改文件。",
    })
    if not discovery.ok then
        report({ error = "discovery failed", status = discovery.status })
        return
    end

    local perspectives = {
        { name = "architecture", focus = "模块边界、依赖方向、公共 API、配置与扩展点" },
        { name = "execution", focus = "CLI/API 到 runtime 的完整执行链路、并发、状态、错误、重试和持久化" },
        { name = "integration", focus = "ACP backend、MCP、daemon、权限、外部客户端和跨进程协议" },
        { name = "developer", focus = "安装、配置、开发流程、测试、示例、故障排查和文档可维护性" },
    }

    phase("inspect", 4)
    local inspections = parallel(perspectives, function(item)
        return {
            name = "documentation-" .. item.name,
            description = "从 " .. item.name .. " 角度收集文档证据",
            prompt = "请只读审查 Luft 仓库 C:\\Users\\heycj\\dev\\luft，不要修改文件。\n\n"
                .. "源码地图：\n" .. text_of(discovery)
                .. "\n\n审查角度：" .. item.focus
                .. "\n\n回到真实源文件逐项核验，输出完整证据清单。每条包括：相对路径、符号/行号、实际行为、对用户或开发者的意义、容易误解的地方、建议写入文档的示例，以及是否受当前未提交修改影响。不要凭空补充源码不存在的功能。",
        }
    end)

    local evidence = {}
    for i, result in ipairs(inspections) do
        evidence[i] = text_of(result)
    end

    phase("draft", 1)
    local draft = agent({
        name = "documentation-draft",
        description = "编写完整的 Luft 技术文档",
        prompt = "你是 Luft 项目的技术文档作者。根据源码地图和多角度证据，为项目编写一份完整、准确、可执行的中文技术文档。\n\n"
            .. "目标文件：" .. document_path .. "\n"
            .. "源码地图：\n" .. text_of(discovery) .. "\n\n"
            .. "独立证据：\n" .. table.concat(evidence, "\n\n--- EVIDENCE ---\n")
            .. "\n\n文档至少包含：\n"
            .. "1. 项目定位、目标、非目标和术语；\n"
            .. "2. 仓库/ crate 结构和职责边界；\n"
            .. "3. 从 CLI、库 API、MCP、daemon 到 scheduler/runtime 的端到端执行链路；\n"
            .. "4. 配置项、环境变量、backend、权限和数据目录；\n"
            .. "5. workflow Lua DSL、agent 会话、MCP 工具和 ACP 集成；\n"
            .. "6. 状态机、事件、checkpoint、恢复、取消、并发和错误处理；\n"
            .. "7. 安装、运行、开发、测试和故障排查步骤；\n"
            .. "8. 可复制的命令/API/Lua 示例；\n"
            .. "9. 当前限制、已知风险和后续演进方向；\n"
            .. "10. 证据索引：尽量给出真实路径、符号和行号。\n\n"
            .. "使用 Write 工具创建或更新目标文件，只修改该目标文档。所有结论必须能回溯到源码或现有配置；不确定的内容明确标注。完成后说明写入的文件和覆盖范围。",
    })
    if not draft.ok then
        report({ error = "draft failed", status = draft.status })
        return
    end

    phase("verify", 1)
    local verification = agent({
        name = "documentation-verify",
        description = "核验完整技术文档",
        prompt = "请审查文档 " .. document_path .. " 与当前 Luft 源码的一致性。只读，不要修改文件。\n"
            .. "逐项检查：引用的路径/符号/行号是否存在；命令、参数、环境变量和 MCP 工具名是否准确；执行链路是否遗漏关键分支；示例是否符合当前 DSL；是否把设计文档中的未来计划误写成现状；是否混入当前未提交修改造成的错误结论。\n"
            .. "输出按 blocker/high/medium/low 分类的问题清单，并给出精确修订建议。若没有问题，明确写出 verified。",
    })

    report({
        workflow = "documentation-complete",
        document = document_path,
        scope = scope,
        discovery = text_of(discovery),
        inspections = evidence,
        draft = text_of(draft),
        verification = text_of(verification),
    })
end
