--------------------------------------------
-- Goal: Analyze, modify, and verify Luft code for a user request
-- Arch:
--   plan ==> implement ==> verify ==> review ==> report
-- Flow: request -> evidence-backed plan -> patch -> tests -> review
--------------------------------------------
-- 按用户需求安全地分析、修改并验证 Luft 代码。
--
-- 参数：
--   args.task         必填，代码修改需求
--   args.scope        可选，限制修改范围

meta = {
    phases = {
        { label = "plan", description = "理解需求、定位代码并形成实施计划", agents = 1 },
        { label = "implement", description = "由单一实现 agent 修改代码和测试", agents = 1 },
        { label = "verify", description = "运行针对性检查并修复实现问题", agents = 1 },
        { label = "review", description = "复查 diff、行为边界和交付摘要", agents = 1 },
    },
    reasoning = "修改代码前先建立源码证据和最小变更计划；实现阶段只保留一个写入 agent，随后由验证 agent 执行测试和必要修复，最后独立复查 diff。",
}

local function text_of(result)
    if result and result.ok then
        return tostring(result.output or "")
    end
    return "[agent failed: " .. tostring(result and result.status or "unknown") .. "]"
end

function main()
    budget(360000, 30)

    local task = (args and args.task) or ""
    local scope = (args and args.scope) or "未指定；根据需求和源码判断最小范围"
    if task == "" then
        report({ error = "args.task is required", usage = "luft run workflows/modify-code.lua --args '{\"task\":\"...\"}'" })
        return
    end

    phase("plan", 1)
    local plan = agent({
        name = "code-change-plan",
        description = "分析代码修改需求并制定计划",
        prompt = "你是 Luft 项目的高级 Rust 工程师。只读检查 C:\\Users\\heycj\\dev\\luft，不要修改文件。\n\n"
            .. "用户需求：\n" .. task .. "\n\n"
            .. "允许/优先范围：" .. scope .. "\n\n"
            .. "使用 rg、git status、git diff、Get-Content 和必要的 cargo metadata 定位真实实现。输出：\n"
            .. "1. 需求拆解和验收标准；\n"
            .. "2. 受影响文件、模块、符号和调用链；\n"
            .. "3. 最小实施步骤；\n"
            .. "4. 需要新增或调整的测试；\n"
            .. "5. 风险、兼容性和不应修改的范围。\n\n"
            .. "不要修改文件，也不要把猜测当成事实。",
    })
    if not plan.ok then
        report({ error = "planning failed", status = plan.status })
        return
    end

    phase("implement", 1)
    local implementation = agent({
        name = "code-change-implement",
        description = "实施代码修改",
        prompt = "你是 Luft 项目的唯一实现 agent。请在 C:\\Users\\heycj\\dev\\luft 中实施下面的用户需求。\n\n"
            .. "用户需求：\n" .. task .. "\n\n"
            .. "修改范围：" .. scope .. "\n\n"
            .. "只读实施计划：\n" .. text_of(plan) .. "\n\n"
            .. "要求：\n"
            .. "- 先检查 git status 和现有 diff，保留用户已有修改，不覆盖无关改动；\n"
            .. "- 只修改实现需求所需的最小文件；\n"
            .. "- 使用 apply_patch 或等价的安全编辑方式；\n"
            .. "- 为行为变化补充或更新针对性测试；\n"
            .. "- 不修改 target、运行时状态目录或凭空重写无关文档；\n"
            .. "- 完成后查看 git diff，说明每个文件的修改和未解决问题。\n\n"
            .. "你可以写入文件，但不要提交 git commit。",
    })
    if not implementation.ok then
        report({ error = "implementation failed", status = implementation.status, plan = text_of(plan) })
        return
    end

    phase("verify", 1)
    local verification = agent({
        name = "code-change-verify",
        description = "验证代码修改并修复问题",
        prompt = "请验证当前 C:\\Users\\heycj\\dev\\luft 中刚完成的代码修改。\n\n"
            .. "用户需求：\n" .. task .. "\n\n"
            .. "实施摘要：\n" .. text_of(implementation) .. "\n\n"
            .. "先查看 git diff 和相关源码，再运行与改动匹配的最小测试（必要时 cargo fmt --check、cargo check、cargo test 或对应 CLI/Lua 校验）。检查错误处理、边界条件、异步/并发行为、兼容性和测试覆盖。发现明确问题时直接修复并重新验证；不要扩大需求范围，不要提交。最后输出测试命令、结果和剩余风险。",
    })

    phase("review", 1)
    local review = agent({
        name = "code-change-review",
        description = "复查最终 diff 和交付结果",
        prompt = "请作为最终 reviewer 检查 Luft 当前工作树。只做必要的小修复，不要提交。\n\n"
            .. "用户需求：\n" .. task .. "\n\n"
            .. "计划：\n" .. text_of(plan) .. "\n\n"
            .. "实现摘要：\n" .. text_of(implementation) .. "\n\n"
            .. "验证摘要：\n" .. text_of(verification) .. "\n\n"
            .. "查看 git diff，确认：需求是否完整满足；是否有越界修改；代码是否符合项目风格；测试是否覆盖核心行为；是否残留明显编译/逻辑风险。输出 ready、needs-follow-up 或 blocked，并列出证据和交付文件。",
    })

    report({
        workflow = "modify-code",
        task = task,
        scope = scope,
        plan = text_of(plan),
        implementation = text_of(implementation),
        verification = text_of(verification),
        review = text_of(review),
    })
end
