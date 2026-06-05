-- converge-demo.lua — 对抗性收敛验证
-- cargo run -- --workflow examples/converge-demo.lua --backend mock --approve

local claims = {
    "API 端点 /users 需要 RBAC 鉴权",
    "密码存储使用了 bcrypt 哈希",
    "输入验证覆盖了 SQL 注入"
}

phase("对抗性验证", #claims * 2)

local result = converge(claims, {
    adversarial = true,
    vote_threshold = 0.7,
    max_rounds = 3
})

if result.converged then
    log(string.format("收敛完成! %d 轮, %d 条 surviving findings",
        result.rounds, #result.findings))
else
    log("未收敛, 达到最大轮次")
end

report(result)