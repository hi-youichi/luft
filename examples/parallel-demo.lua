-- parallel-demo.lua — 并行处理多个任务
-- cargo run -- run --workflow examples/parallel-demo.lua --backend mock

meta = {
  reasoning = "Demonstrate parallel processing of multiple files with the parallel primitive",
  phases = {
    { label = "并行审查", dynamic = false },
  },
}

local files = { "src/main.rs", "src/lib.rs", "src/cli.rs" }

function main()
    phase("并行审查", #files)

    local results = parallel(files, function(file)
        return {
            prompt = "审查这个文件: " .. file
        }
    end)

    local findings_count = 0
    for i, r in ipairs(results) do
        log(string.format("  %s → %s (%d tokens)", files[i], r.status, r.tokens))
        findings_count = findings_count + #r.findings
    end

    report({
        total_files = #files,
        total_findings = findings_count,
        results = results
    })
end