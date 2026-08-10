--------------------------------------------
-- Goal: Produce a complete, source-grounded readability improvement document for Luft
-- Arch:
--   discover ==> inspect in parallel ==> draft ==> verify ==> report
-- Flow: repository map -> evidence findings[] -> draft document -> verified document
--------------------------------------------
meta = {
  reasoning = "Trace the current repository first, obtain independent readability reviews, draft a complete design document, then verify every recommendation against source evidence.",
  phases = {
    { label = "discover", description = "盘点仓库结构、入口和当前工作树", agents = 1 },
    { label = "inspect", description = "并行审查架构、控制流和开发者体验", agents = 3, dynamic = true },
    { label = "draft", description = "生成完整的可读性改进文档", agents = 1 },
    { label = "verify", description = "核验文档中的证据与实施边界", agents = 1 },
    { label = "report", description = "返回文档路径与审查摘要", agents = 0 },
  },
}

local function output_of(result)
  if result and result.ok then
    return tostring(result.output or "")
  end
  return "[agent failed: " .. tostring(result and result.status or "unknown") .. "]"
end

function main()
  budget(360000, 40)
  local document_path = (args and args.document_path) or "docs/design/readability-improvement-plan.md"

  phase("discover", 1)
  local discovery = agent({
    name = "readability-discover",
    description = "Map the repository and current working tree",
    prompt = "你是 Luft 项目的仓库分析员。只读检查当前工作区 C:\\Users\\heycj\\dev\\luft。使用 rg、git status、git diff --stat、Get-Content 等工具，盘点 workspace/crates、CLI 入口、Luft facade、runtime/scheduler、journal/storage、MCP/service、测试和现有 docs。必须区分当前工作树未提交修改与基线代码，不要修改文件。输出普通文本，包含真实相对路径、符号、行号、主要执行链路、文档现状和可读性热点。"
  })
  if not discovery.ok then
    report({ error = "discovery failed", status = discovery.status })
    return
  end

  local perspectives = {
    { name = "architecture", focus = "模块边界、公共 API、职责分离、重复模型和跨 crate 依赖" },
    { name = "execution-flow", focus = "CLI 到 runtime 的控制流、异步并发、状态/错误/重试与日志" },
    { name = "onboarding", focus = "命名、测试、注释、设计文档、示例和新贡献者理解路径" },
  }

  phase("inspect", 3)
  local reviews = parallel(perspectives, function(item)
    return {
      name = "readability-" .. item.name,
      description = "Review readability: " .. item.name,
      prompt = "请只读审查 Luft 项目 C:\\Users\\heycj\\dev\\luft，不要修改文件。\n\n仓库盘点：\n"
        .. output_of(discovery)
        .. "\n\n你的审查角度：" .. item.focus
        .. "\n\n请回到真实源文件核验，输出 5-12 条高价值发现。每条包括：P0/P1/P2 优先级、相对路径、符号或行号、代码证据、理解成本、具体建议、收益、成本/行为风险。明确哪些问题来自当前未提交改动，哪些属于基线问题。不要把纯风格偏好当成缺陷。"
    }
  end)

  local review_text = {}
  for i, review in ipairs(reviews) do
    review_text[i] = output_of(review)
  end

  phase("draft", 1)
  local draft = agent({
    name = "readability-draft",
    description = "Write the complete readability improvement plan",
    prompt = "你是技术设计文档作者。请基于仓库盘点和三份独立审查，为 Luft 编写一份完整、可执行、基于源码证据的中文可读性改进文档。\n\n仓库盘点：\n"
      .. output_of(discovery)
      .. "\n\n独立审查：\n"
      .. table.concat(review_text, "\n\n--- REVIEW ---\n")
      .. "\n\n目标文件：" .. document_path
      .. "\n\n文档必须包含：\n"
      .. "1. 背景、目标、非目标和当前工作树说明；\n"
      .. "2. 当前架构与核心执行链路；\n"
      .. "3. 可读性问题总览表（优先级、证据、影响、范围）；\n"
      .. "4. 分主题详细方案：模块边界/API、控制流/错误处理、状态与持久化、CLI/MCP/service、测试/文档/命名；\n"
      .. "5. 每项方案的 before/after 结构示意、拟修改文件、行为不变量、风险和验证方式；\n"
      .. "6. quick wins、局部重构、架构级改动三阶段路线图；\n"
      .. "7. 不建议现在做的改动；\n"
      .. "8. 验收标准、测试策略、回滚策略和待用户决策的问题；\n"
      .. "9. 附录：证据索引，路径和行号尽量精确。\n\n"
      .. "只写真实代码支持的结论。请使用 Write 工具创建或更新目标文件；不要修改其他文件。完成后用普通文本说明写入了什么。"
  })
  if not draft.ok then
    report({ error = "draft failed", status = draft.status })
    return
  end

  phase("verify", 1)
  local verification = agent({
    name = "readability-doc-verify",
    description = "Verify the generated readability document",
    prompt = "请审查文档 " .. document_path .. " 与当前 Luft 源码的一致性。只读，不要修改文件。逐项检查：引用的路径/符号/行号是否存在；建议是否与当前未提交工作树相冲突；是否遗漏关键执行链路；是否把风格偏好误写成问题；实施顺序和测试验收是否可执行。输出问题清单，按 blocker/high/medium/low 分类，并给出精确修订建议。"
  })

  report({
    workflow = "readability-documentation",
    document = document_path,
    discovery = output_of(discovery),
    reviews = review_text,
    draft = output_of(draft),
    verification = output_of(verification),
  })
end
