-- model-compare.lua — 同一个 prompt，并行跑多个模型做对比
--
-- cargo run -- run -w examples/model-compare.lua -b mock \
--     --log .luft/example_logs/model-compare.jsonl --log-format jsonl
--
-- 换成真实后端（需要对应模型可用）：
-- cargo run -- run -w examples/model-compare.lua -b claude-acp \
--     --log .luft/example_logs/model-compare.jsonl --log-format jsonl
--
-- 每个 agent() 调用可以带独立的 `model` 字段覆盖本次调用用的模型
-- （backend 本身仍由 --backend 统一指定，一次 run 内不能混用多个 backend）。
--
-- 怎么知道请求的 model 是不是真的生效了？luft 目前不会校验/暴露"确认生效的
-- model"（`session/set_config_option` 的响应被丢弃了，见
-- docs/architecture/adapters.md §7），所以这里用最直接的笨办法：让模型自己在
-- 回答里报出具体的模型 ID，人工核对 `results[i].output.text` 里报的名字是否
-- 跟 `results[i].model`（请求的那个）一致。另一个办法是翻 `--log-format
-- jsonl` 产出的 acp_raw 事件，grep 里面有没有 "model" 字段。

meta = {
    reasoning = "Compare how different models answer the exact same prompt",
    phases = {
        { label = "compare_models", dynamic = false },
    },
}

local MODELS = {
    "claude-opus-4-8",
    "claude-sonnet-5",
    "claude-haiku-4-5-20251001",
}

local PROMPT = "Explain recursion in exactly one sentence. "
    .. "Then, on a new line, state your exact model identifier "
    .. "(the precise model ID/version string you know yourself as)."

function main()
    phase("compare_models", #MODELS)

    local results = parallel(MODELS, function(model)
        return {
            prompt = PROMPT,
            model = model,
        }
    end)

    local rows = {}
    for i, model in ipairs(MODELS) do
        local r = results[i]
        local text = (r.output and r.output.text) or r.output
        log(string.format("  %-30s -> %s (%d tokens): %s", model, r.status, r.tokens, tostring(text)))
        rows[i] = {
            model = model,
            status = r.status,
            ok = r.ok,
            tokens = r.tokens,
            output = r.output,
        }
    end

    report({
        prompt = PROMPT,
        models_tested = #MODELS,
        results = rows,
    })
end
