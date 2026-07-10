-- schema-simple.lua — 最简 schema 演示
-- cargo run --bin luft -- run -w examples/schema-simple.lua -b opencode
-- cargo run --bin luft -- run -w examples/schema-simple.lua -b mock (降级)
--
-- 核心: 1 个 schema + 1 次 agent() 调用，最小化演示 JSON Schema 约束

meta = {
  reasoning = "Minimal demonstration of JSON Schema constrained structured output",
  phases = {
    { label = "提取书籍信息", description = "从已知书名提取结构化书籍数据", dynamic = false },
  },
}

local BOOK_SCHEMA = {
  type = "object",
  properties = {
    title = { type = "string" },
    author = { type = "string" },
    year = { type = "integer" },
    tags = { type = "array", items = { type = "string" } },
  },
  required = { "title", "author" },
}

function main()
  phase("提取书籍信息", 1)

  local ok, res = pcall(agent, {
    name = "book_extractor",
    description = "Extract structured book information from a known title",
    role = "librarian",
    prompt = "Introduce the book 'The Pragmatic Programmer' by Andy Hunt and Dave Thomas. Return structured data only.",
    schema = BOOK_SCHEMA,
  })

  if not ok or type(res) ~= "table" then
    log("降级到 mock 数据", "warn")
    report({
      ok = false,
      source = "mock_fallback",
      info = { title = "Mock Book", author = "Mock Author" },
    })
    return
  end

  log(string.format("《%s》 — %s (%d)", res.output.title, res.output.author, res.output.year or 0))
  report({
    ok = true,
    source = "structured_output",
    info = res.output,
  })
end
