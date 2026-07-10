-- hello.lua — 最简 agent 调用
-- cargo run -- run --workflow examples/hello.lua --backend mock

meta = {
    reasoning = "Simplest possible agent call demonstration",
    phases = {
        { label = "hello", dynamic = false },
    },
}

function main()
    local result = agent({
        prompt = "Say hello in exactly 3 words",
    })

    log("status: " .. result.status)
    log("output: " .. json.encode(result.output))

    report({
        status = result.status,
        output = result.output,
        tokens = result.tokens
    })
end
