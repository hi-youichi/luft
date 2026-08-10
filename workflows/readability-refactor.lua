--------------------------------------------
-- Goal: Implement and verify approved readability improvements from a design document
-- Arch:
--   read plan ==> select safe scope ==> implement ==> test ==> review ==> report
-- Flow: design document -> approved change set -> patch -> validation -> change report
--------------------------------------------
meta = {
  reasoning = "Read the approved readability plan, constrain the change set to explicit low-risk items, implement in small patches, run focused verification, and report any remaining decisions.",
  phases = {
    { label = "plan", description = "读取设计文档并确定本轮实施范围", agents = 1 },
    { label = "implement", description = "按小步提交可读性改动", agents = 1 },
    { label = "test", description = "运行格式化、编译和聚焦测试", agents = 1 },
    { label = "review", description = "复查 diff、行为不变量和未覆盖风险", agents = 1 },
    { label = "report", description = "返回变更、验证结果和后续决策", agents = 0 },
  },
}

local function output_of(result)
  if result and result.ok then
    return tostring(result.output or "")
  end
  return "[agent failed: " .. tostring(result and result.status or "unknown") .. "]"
end

function main()
  budget(420000, 50)
  local document_path = (args and args.document_path) or "docs/design/readability-improvement-plan.md"
  local scope = (args and args.scope) or "P0/P1 quick wins and local refactors only; do not perform architecture-level changes without explicit approval"

  phase("plan", 1)
  local plan = agent({
    name = "readability-refactor-plan",
    description = "Select an implementation scope from the readability plan",
    prompt = "请阅读设计文档 " .. document_path .. "、当前工作区 git status 和相关源码。只读，不要修改文件。实施约束：" .. scope
      .. "。请列出本轮准确的修改项：目标文件、目标符号、修改前后意图、不得改变的行为、所需测试和潜在冲突。必须特别标记与当前未提交修改重叠的文件，并建议如何安全处理。输出普通文本。"
  })
  if not plan.ok then
    report({ error = "plan failed", status = plan.status })
    return
  end

  phase("implement", 1)
  local implementation = agent({
    name = "readability-refactor-implement",
    description = "Implement the approved readability changes",
    prompt = "你是 Luft 项目的实现代理。阅读设计文档 " .. document_path .. " 和实施范围分析：\n\n"
      .. output_of(plan)
      .. "\n\n请在当前工作区实施明确且低风险的可读性改动。\n"
      .. "要求：\n"
      .. "- 先读取目标文件和 git diff；保留用户已有未提交修改，不得 reset、checkout、clean 或覆盖无关改动；\n"
      .. "- 只修改实施范围明确列出的文件；若发现范围不足或会改变行为，停止该项并说明原因；\n"
      .. "- 使用 apply_patch/Write 等编辑工具，优先小而清晰的补丁；\n"
      .. "- 不为了减少行数而牺牲命名、错误上下文或 API 语义；\n"
      .. "- 对公共 Rust API、错误类型、异步生命周期和持久化语义保持兼容，除非文档明确批准；\n"
      .. "- 运行必要的 rustfmt 或等价格式化，但不要改写无关文件。\n\n"
      .. "完成后输出修改文件、关键改动、未实施项和原因。"
  })
  if not implementation.ok then
    report({ error = "implementation failed", status = implementation.status, plan = output_of(plan) })
    return
  end

  phase("test", 1)
  local tests = agent({
    name = "readability-refactor-test",
    description = "Run focused validation for the readability changes",
    prompt = "请验证刚才对 Luft 的代码改动。先检查 git diff 和修改文件，再根据实际 crate 选择最小充分的验证：cargo fmt --check 或 cargo fmt --check --all；cargo check -p <affected crates>；相关 cargo test -p <affected crates>。必要时运行 clippy，但不要修改源码。不要把已有失败误报为本次回归：比较错误与改动的关系。输出每个命令、结果、失败原因和是否与本次改动相关。只读分析和运行验证，不再编辑文件。\n\n实现代理报告：\n" .. output_of(implementation)
  })

  phase("review", 1)
  local review = agent({
    name = "readability-refactor-review",
    description = "Review the final diff and implementation risks",
    prompt = "请做最终只读 diff review。检查当前 git diff 与设计文档 " .. document_path .. "：\n"
      .. "1. 是否只实现了批准范围；\n2. 是否保留行为、错误语义、并发和持久化不变量；\n3. 命名和抽象是否真的提高可读性；\n4. 是否引入重复、过度抽象或无关格式化；\n5. 测试结果是否足够；\n6. 当前未提交改动是否被误伤。\n\n实施报告：\n" .. output_of(implementation)
      .. "\n\n验证报告：\n" .. output_of(tests)
      .. "\n\n按 blocker/high/medium/low 输出问题，并给出是否可以合并/继续讨论的结论。不要修改文件。"
  })

  report({
    workflow = "readability-refactor",
    document = document_path,
    scope = scope,
    plan = output_of(plan),
    implementation = output_of(implementation),
    tests = output_of(tests),
    review = output_of(review),
  })
end
