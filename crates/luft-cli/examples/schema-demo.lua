-- schema-demo.lua — Schema-constrained structured output 演示
-- cargo run --bin maestro -- run -w examples/schema-demo.lua -b opencode --approve
-- cargo run --bin maestro -- run -w examples/schema-demo.lua -b mock (仅验证接线, schema 需 opencode)
--
-- 演示 agent() / parallel() 中使用 schema 参数约束 LLM 输出结构，
-- 以及结构化数据的定义、传递和组合。
--
-- 设计哲学: 每个 agent 调用只输出一个小结构，由编排层组合。
--   extract -> parallel(validate) -> summarize

meta = {
  reasoning = "Demonstrate structured output via JSON Schema constraints with multi-agent data processing",
  phases = {
    { label = "schema 演示", dynamic = false },
  },
}

----------------------------------------------------------------------
-- Schemas
----------------------------------------------------------------------

local PERSON_SCHEMA = {
  type = "object",
  properties = {
    name = { type = "string" },
    role = { type = "string" },
    languages = { type = "array", items = { type = "string" } },
    yoe = { type = "integer" },
  },
  required = { "name", "role", "languages" },
}

local FINDING_SCHEMA = {
  type = "object",
  properties = {
    approved = { type = "boolean" },
    score = { type = "integer" },
    comment = { type = "string" },
  },
  required = { "approved", "score" },
}

local REPORT_SCHEMA = {
  type = "object",
  properties = {
    total = { type = "integer" },
    approved = { type = "integer" },
    avg_score = { type = "number" },
    summary = { type = "string" },
    top_contributor = { type = "string" },
  },
  required = { "total", "approved", "avg_score", "summary" },
}

----------------------------------------------------------------------
-- safe_agent: 用 pcall 包装 agent，使其在 mock 后端下也能正常完成
----------------------------------------------------------------------

local function safe_agent(opts)
  local ok, res = pcall(agent, opts)
  if ok and type(res) == "table" then return res end
  log("Agent 调用降级: " .. tostring(res), "warn")
  return { status = "error", ok = false, output = {}, tokens = 0 }
end

----------------------------------------------------------------------
-- Main
----------------------------------------------------------------------

function main()
  phase("schema 演示", 4)
  log("开始 schema 结构化输出演示...")

  -- 1. 用 PERSON_SCHEMA 约束输出 → extract.output.name / .role / .languages / .yoe
  local extract = safe_agent({
    name = "contributor_extractor",
    description = "提取 Maestro 项目的虚构贡献者信息",
    role = "analyst",
    prompt = table.concat({
      "You are analyzing the Maestro project contributors.",
      "List 3 fictional contributors with their name, role, programming languages, and years of experience.",
      "Be specific and realistic (Rust, Lua, TypeScript).",
      "Return ONLY the structured output via the structured_output tool.",
    }, "\n"),
    schema = PERSON_SCHEMA,
  })

  if not extract.ok then
    log("extract 失败, 使用降级数据", "warn")
    extract = { ok = true, output = {
      name = "Fallback Developer", role = "降级工程师",
      languages = { "Rust" }, yoe = 5,
    }}
  end

  log(string.format("提取到: %s (%s, %d 年经验, %s)",
    extract.output.name,
    extract.output.role,
    extract.output.yoe or 0,
    table.concat(extract.output.languages or {}, ", ")))

  -- 2. parallel(items, mapperFn) 中每个 item 由 mapper 转成 agent opts
  --    mapperFn 的返回值可携带 schema，每个 agent 独立输出 { approved, score, comment }
  local inputs = { 1, 2, 3 }
  local results = parallel(inputs, function(i)
    return {
      name = "assessor_" .. i,
      description = "评估贡献者是否适合担任 team lead",
      role = "reviewer",
      prompt = table.concat({
        "Assess this contributor as a team lead:",
        json.encode({ name = extract.output.name, role = extract.output.role }),
        "",
        "Return approved=true if they are suitable (yoe >= 3), score 1-10, and a short comment.",
        "Use the structured_output tool.",
      }, "\n"),
      schema = FINDING_SCHEMA,
    }
  end)

  -- 3. 聚合 parallel 结果（按字段访问结构化输出）
  local approved = 0
  local total_score = 0
  for _, r in ipairs(results) do
    if r.ok and r.output and r.output.approved then
      approved = approved + 1
    end
    if r.ok and r.output and r.output.score then
      total_score = total_score + r.output.score
    end
  end

  log(string.format("评估: %d/%d 通过, 平均分 %.1f",
    approved, #results, #results > 0 and total_score / #results or 0))

  -- 4. 用 REPORT_SCHEMA 生成最终报告
  local summary = safe_agent({
    name = "report_generator",
    description = "生成贡献者评估总结报告",
    role = "analyst",
    prompt = table.concat({
      "Generate a brief one-paragraph summary in CHINESE for a contributor assessment report.",
      "Details:",
      json.encode({
        candidate = extract.output.name,
        role = extract.output.role,
        approved = approved,
        total = #results,
        avg_score = #results > 0 and math.floor(total_score / #results) or 0,
      }),
    }, "\n"),
    schema = REPORT_SCHEMA,
  })

  local report_data = {
    extracted = extract.output,
    assessments = results,
    eval = {
      approved = approved,
      total = #results,
      avg_score = #results > 0 and total_score / #results or 0,
    },
  }

  if summary.ok and summary.output then
    report_data.summary = summary.output.summary or "(no summary)"
    report_data.avg_score = summary.output.avg_score
    report_data.top_contributor = summary.output.top_contributor
  else
    report_data.summary = "(summary generation failed)"
  end

  report(report_data)
end
