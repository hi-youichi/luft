-- pipeline-demo.lua — 多阶段流式管道
-- cargo run -- run --workflow examples/pipeline-demo.lua --backend mock

local topics = { "Rust async runtime", "Lua VM embedding", "MCP protocol" }

local result = pipeline(topics, {
    {
        name = "research",
        handler = function(item)
            return agent({ prompt = "深入研究: " .. item })
        end
    },
    {
        name = "summarize",
        handler = function(item, prev)
            return agent({
                prompt = "用 3 句话总结: " .. json.encode(prev.output)
            })
        end
    }
})

log(string.format("pipeline: %d/%d 成功, %d 阶段, %dms",
    result.ok, result.ok + result.failed,
    result.total_stages, result.total_elapsed_ms))

report(result)