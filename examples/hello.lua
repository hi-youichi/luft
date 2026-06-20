-- hello.lua — 最简 agent 调用
-- cargo run -- run --workflow examples/hello.lua --backend mock

local result = agent({
    prompt = "Say hello in exactly 3 words",
    model = "mock"
})

log("status: " .. result.status)
log("output: " .. json.encode(result.output))

report({
    status = result.status,
    output = result.output,
    tokens = result.tokens
})