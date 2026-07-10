-- tech-haiku.lua — 技术俳句生成器
-- 两阶段流水线：生成 → 评审优化 → 结构化报告
--
-- 运行方式:
--   cargo run -- run -w workflows/tech-haiku.lua -b mock
--   cargo run -- run -w workflows/tech-haiku.lua -b opencode

meta = {
  reasoning = "两阶段技术俳句工作流：先生成初稿，再由评审 agent 优化，最终输出结构化结果",
  phases = {
    { label = "生成初稿", description = "根据主题生成技术俳句初稿", dynamic = false },
    { label = "评审优化", description = "评审初稿并输出优化后的俳句",  dynamic = false },
    { label = "汇总报告", description = "汇总前后版本并生成最终报告",  dynamic = false },
  },
}

-- 结构化输出 Schema
local HAIKU_SCHEMA = {
  type = "object",
  properties = {
    line1 = { type = "string", description = "俳句第一行（5音）" },
    line2 = { type = "string", description = "俳句第二行（7音）" },
    line3 = { type = "string", description = "俳句第三行（5音）" },
    theme = { type = "string", description = "主题关键词" },
  },
  required = { "line1", "line2", "line3" },
}

function main()
  local topic = args.topic or "Rust 编译器"

  -----------------------------------------------------------
  -- Phase 1: 生成初稿
  -----------------------------------------------------------
  phase("生成初稿", 1)
  log("📝 主题: " .. topic)

  local draft_ok, draft = pcall(agent, {
    name = "haiku_poet",
    description = "根据技术主题创作俳句",
    role = "你是一位热爱编程的诗人",
    prompt = string.format(
      "请以「%s」为主题创作一首技术俳句。俳句共三行，遵循 5-7-5 音节结构。"
      .. "要求幽默、有技术深度。只返回结构化数据。",
      topic
    ),
    schema = HAIKU_SCHEMA,
  })

  if not draft_ok or type(draft) ~= "table" or not draft.output then
    log("初稿生成失败，降级处理", "warn")
    report({ ok = false, error = "draft_generation_failed", topic = topic })
    return
  end

  local draft_text = string.format("%s / %s / %s",
    draft.output.line1, draft.output.line2, draft.output.line3)
  log("✏️ 初稿: " .. draft_text)

  -----------------------------------------------------------
  -- Phase 2: 评审优化
  -----------------------------------------------------------
  phase("评审优化", 1)

  local review_ok, reviewed = pcall(agent, {
    name = "haiku_critic",
    description = "评审并优化技术俳句",
    role = "你是一位严格的文学编辑，精通技术文化",
    prompt = string.format(
      "请评审这首技术俳句，并在保持 5-7-5 结构的前提下进行优化：\n\n%s\n\n"
      .. "主题: %s\n"
      .. "要求：更有意境、更有技术梗。只返回优化后的结构化数据。",
      draft_text, topic
    ),
    schema = HAIKU_SCHEMA,
  })

  local final_haiku
  if review_ok and type(reviewed) == "table" and reviewed.output then
    final_haiku = reviewed.output
    log("🔧 优化: " .. string.format("%s / %s / %s",
      final_haiku.line1, final_haiku.line2, final_haiku.line3))
  else
    log("评审失败，使用初稿作为最终结果", "warn")
    final_haiku = draft.output
  end

  -----------------------------------------------------------
  -- Phase 3: 汇总报告
  -----------------------------------------------------------
  phase("汇总报告", 1)

  report({
    ok = true,
    topic = topic,
    draft = draft.output,
    final = final_haiku,
    improved = review_ok and (draft.output.line1 ~= final_haiku.line1) or false,
    tokens_used = (draft.tokens or 0) + (reviewed and (reviewed.tokens or 0) or 0),
  })
end
