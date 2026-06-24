-- phased_hello.lua — minimal fixture demonstrating the meta + main() format.
-- Run: cargo run -- run --workflow examples/phased_hello.lua --backend mock

meta = {
    phases = {
        { label = "prepare",   detail = "Set up a greeting",       agents = 1, depends_on = {} },
        { label = "agent_run", detail = "Run the hello agent",     agents = 1, depends_on = { 1 } },
        { label = "report",    detail = "Report the final output", agents = 1, depends_on = { 2 } }
    },
    reasoning = "Three simple phases to exercise the meta extraction and phases view"
}

function main()
    phase("prepare", 1)

    phase("agent_run", 1)
    local result = agent({
        prompt = "Say hello in exactly 3 words",
        model = "mock"
    })
    log("status: " .. result.status)
    log("output: " .. json.encode(result.output))

    phase("report", 1)
    report({
        status = result.status,
        output = result.output,
        tokens = result.tokens
    })
end