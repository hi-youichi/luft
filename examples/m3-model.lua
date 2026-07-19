-- m3-model.lua - agent primitive with model = "MiniMax-M3" (minimal example)
-- cargo run --bin luft -- run --workflow examples/m3-model.lua --backend mock

meta = {
    reasoning = "Demonstrates agent({ model = ... }) field; mock backend so no real LLM call is needed.",
    phases = {
        { label = "hello-m3", dynamic = false },
    },
}

function main()
    local r = agent({
        prompt = "Say hello in one sentence.",
        model = "MiniMax-M3",
    })

    log("status: " .. tostring(r.status))
    log("output: " .. json.encode(r.output))

    report({
        status = r.status,
        output = r.output,
        tokens = r.tokens,
    })
end
