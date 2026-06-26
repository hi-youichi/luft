import type {
  RunSummary, RunCheckpoint, AgentEvent, DashboardStats,
  WorkflowSummary, WorkflowDetail, BackendConfig, AgentResultCache,
} from './types'

const now = Date.now()
const min = 60_000

function ts(minutesAgo: number): string {
  return new Date(now - minutesAgo * min).toISOString()
}

function elapsed(minute: number): number {
  return minute * 60_000
}

// ── Runs ──

export const mockRuns: RunSummary[] = [
  {
    run_id: 'r4f2a001', run_dir: '2025-08-19_r4f2a001', task: '分析 src/ 目录代码质量',
    status: 'running', current_phase: 2, total_phases: 3, total_tokens: 2600,
    started_at: ts(3.2), elapsed_ms: elapsed(3.2),
  },
  {
    run_id: 'r4f2a002', run_dir: '2025-08-19_r4f2a002', task: '安全审计：检查依赖漏洞',
    status: 'running', current_phase: 1, total_phases: 4, total_tokens: 890,
    started_at: ts(1.7), elapsed_ms: elapsed(1.7),
  },
  {
    run_id: 'r4f2a003', run_dir: '2025-08-19_r4f2a003', task: '生成测试用例',
    status: 'completed', current_phase: 3, total_phases: 3, total_tokens: 8400,
    started_at: ts(23), elapsed_ms: elapsed(8),
  },
  {
    run_id: 'r4f2a004', run_dir: '2025-08-19_r4f2a004', task: '部署检查',
    status: 'failed', current_phase: 2, total_phases: 5, total_tokens: 3100,
    started_at: ts(35), elapsed_ms: elapsed(12),
  },
  {
    run_id: 'r4f2a005', run_dir: '2025-08-18_r4f2a005', task: '文档审查与补全',
    status: 'completed', current_phase: 4, total_phases: 4, total_tokens: 12000,
    started_at: ts(180), elapsed_ms: elapsed(28),
  },
  {
    run_id: 'r4f2a006', run_dir: '2025-08-18_r4f2a006', task: 'API 接口设计审查',
    status: 'completed', current_phase: 2, total_phases: 2, total_tokens: 5600,
    started_at: ts(300), elapsed_ms: elapsed(15),
  },
  {
    run_id: 'r4f2a007', run_dir: '2025-08-18_r4f2a007', task: '数据库迁移脚本验证',
    status: 'cancelled', current_phase: 1, total_phases: 3, total_tokens: 1200,
    started_at: ts(420), elapsed_ms: elapsed(4),
  },
  {
    run_id: 'r4f2a008', run_dir: '2025-08-18_r4f2a008', task: '前端性能分析',
    status: 'completed', current_phase: 3, total_phases: 3, total_tokens: 9800,
    started_at: ts(600), elapsed_ms: elapsed(22),
  },
  {
    run_id: 'r4f2a009', run_dir: '2025-08-17_r4f2a009', task: 'CI/CD 流水线优化',
    status: 'completed', current_phase: 5, total_phases: 5, total_tokens: 15400,
    started_at: ts(1440), elapsed_ms: elapsed(45),
  },
  {
    run_id: 'r4f2a010', run_dir: '2025-08-17_r4f2a010', task: '代码重构：提取公共模块',
    status: 'failed', current_phase: 3, total_phases: 4, total_tokens: 6700,
    started_at: ts(1500), elapsed_ms: elapsed(18),
  },
  {
    run_id: 'r4f2a011', run_dir: '2025-08-17_r4f2a011', task: '日志系统升级',
    status: 'completed', current_phase: 2, total_phases: 2, total_tokens: 4200,
    started_at: tx(1620), elapsed_ms: elapsed(11),
  },
  {
    run_id: 'r4f2a012', run_dir: '2025-08-16_r4f2a012', task: 'WebSocket 实时通信实现',
    status: 'completed', current_phase: 4, total_phases: 4, total_tokens: 11200,
    started_at: ts(2880), elapsed_ms: elapsed(35),
  },
]

function tx(minAgo: number): string { return ts(minAgo) }

// ── Agent helpers ──

function agent(id: string, role: AgentResultCache['role'], status: AgentResultCache['status'], inputTok: number, outputTok: number, elapsedMin: number, prompt: string, output: string, description: string, toolCalls: number, error?: string): AgentResultCache {
  return { agent_id: id, role, status, tokens: { input: inputTok, output: outputTok }, elapsed_ms: elapsedMin * 60_000, prompt_preview: prompt, output_preview: output, description, tool_calls: toolCalls, error }
}

// ── Checkpoints ──

export const mockCheckpoints: Record<string, RunCheckpoint> = {
  r4f2a001: {
    run_id: 'r4f2a001', task: '分析 src/ 目录代码质量', status: 'running', current_phase: 2,
    started_at: ts(3.2), elapsed_ms: elapsed(3.2),
    total_tokens: 2600, total_tokens_detail: { input: 1800, output: 800 },
    phases: [
      {
        phase_id: 1, label: '生成分析', status: 'completed', role: 'producer',
        description: '使用多个 producer agent 并行分析代码质量，产出结构化 findings',
        agents: [
          agent('agent_1', 'producer', 'done', 500, 700, 0.2, '分析 src/core/ 模块的代码质量', 'core/ 模块结构清晰，但 state.rs 存在职责过重问题，建议拆分。', 'core/ 模块代码分析', 4),
          agent('agent_2', 'producer', 'done', 400, 580, 0.13, '分析 src/runtime/ 模块的代码质量', 'runtime/ 模块整体良好，sandbox.rs 的错误处理路径有改进空间。', 'runtime/ 模块代码分析', 3),
          agent('agent_3', 'producer', 'done', 300, 120, 0.08, '分析 src/cli.rs 模块的代码质量', 'CLI 模块简洁，无明显问题。', 'cli.rs 模块代码分析', 2),
        ],
      },
      {
        phase_id: 2, label: '对抗验证', status: 'running', role: 'adversary',
        description: 'adversary agent 挑战 Phase 1 结论，找出不准确或不完整的部分',
        agents: [
          agent('agent_4', 'producer', 'done', 350, 630, 0.13, '验证 core/ 分析结果的准确性', '确认 state.rs 的职责拆分建议合理，补充了具体拆分方案。', '验证 core/ 分析结果', 5),
          agent('agent_5', 'adversary', 'running', 200, 140, 0.07, '挑战 core/ 分析结论', '生成中...', '挑战 core/ 分析结论', 1),
        ],
      },
      {
        phase_id: 3, label: '投票收敛', status: 'pending', role: 'voter',
        description: 'voter agent 综合所有分析和对抗结果，产出最终报告',
        agents: [],
      },
    ],
    findings: [
      { id: 'f1', severity: 'high', message: 'state.rs 职责过重（78% 覆盖率但单文件过大），建议拆分为 state/mod.rs + state/progress.rs', source: 'agent_1' },
      { id: 'f2', severity: 'medium', message: 'sandbox.rs 的超时处理路径缺少日志记录', source: 'agent_2' },
      { id: 'f3', severity: 'low', message: 'cli.rs 的 help 文本可以更详细', source: 'agent_3' },
    ],
  },
  r4f2a003: {
    run_id: 'r4f2a003', task: '生成测试用例', status: 'completed', current_phase: 3,
    started_at: ts(23), elapsed_ms: elapsed(8),
    total_tokens: 8400, total_tokens_detail: { input: 5200, output: 3200 },
    phases: [
      {
        phase_id: 1, label: '分析现有测试', status: 'completed', role: 'producer',
        agents: [agent('a1', 'producer', 'done', 800, 600, 1.5, '扫描现有测试文件并分析覆盖率', '当前覆盖率 65%，主要盲区在 TUI 层和 converge.rs', '扫描现有测试覆盖率', 5)],
      },
      {
        phase_id: 2, label: '生成测试', status: 'completed', role: 'producer',
        agents: [
          agent('a2', 'producer', 'done', 1200, 900, 2.5, '为 converge.rs 生成单元测试', '生成 12 个测试用例，覆盖正常/边界/错误路径。', '为 converge.rs 生成测试', 6),
          agent('a3', 'producer', 'done', 1000, 800, 2.0, '为 sandbox.rs 生成单元测试', '生成 8 个测试用例，覆盖超时/OOM/退出码场景。', '为 sandbox.rs 生成测试', 4),
        ],
      },
      {
        phase_id: 3, label: '验证测试', status: 'completed', role: 'voter',
        agents: [agent('a4', 'voter', 'done', 600, 400, 1.0, '验证生成的测试是否通过', '20 个测试全部通过，无回归。', '验证生成的测试通过', 3)],
      },
    ],
    findings: [
      { id: 'f1', severity: 'medium', message: 'converge.rs 收敛逻辑的边界条件缺少测试', source: 'a2' },
      { id: 'f2', severity: 'low', message: '建议增加 sandbox panic 恢复测试', source: 'a3' },
    ],
  },
  r4f2a004: {
    run_id: 'r4f2a004', task: '部署检查', status: 'failed', current_phase: 2,
    started_at: ts(35), elapsed_ms: elapsed(12),
    total_tokens: 3100, total_tokens_detail: { input: 2200, output: 900 },
    phases: [
      {
        phase_id: 1, label: '环境检查', status: 'completed', role: 'producer',
        agents: [agent('a1', 'producer', 'done', 600, 400, 2, '检查部署环境配置', '环境配置正确，Docker 镜像可用。', '检查部署环境配置', 3)],
      },
      {
        phase_id: 2, label: '预部署验证', status: 'completed', role: 'producer',
        agents: [
          agent('a2', 'producer', 'done', 800, 300, 3, '验证数据库迁移脚本', '', '验证数据库迁移脚本', 4, '迁移脚本执行失败：外键约束冲突，table `runs` 引用了不存在的 `backends.id`'),
          agent('a3', 'producer', 'failed', 500, 0, 2, '验证 API 健康检查', '', '验证 API 健康检查', 3, 'API 启动超时，端口 3000 被占用'),
        ],
      },
    ],
    findings: [
      { id: 'f1', severity: 'critical', message: '数据库迁移失败：外键约束冲突', source: 'a2' },
      { id: 'f2', severity: 'high', message: '端口 3000 被占用，服务无法启动', source: 'a3' },
    ],
  },
  r4f2a005: {
    run_id: 'r4f2a005', task: '文档审查与补全', status: 'completed', current_phase: 4,
    started_at: ts(180), elapsed_ms: elapsed(28),
    total_tokens: 12000, total_tokens_detail: { input: 7000, output: 5000 },
    phases: [
      { phase_id: 1, label: '扫描文档', status: 'completed', role: 'producer', agents: [agent('a1', 'producer', 'done', 1000, 800, 5, '扫描所有 .md 文件', '找到 45 个文档文件，其中 12 个缺少内容。', '扫描文档文件', 8)] },
      { phase_id: 2, label: '审查质量', status: 'completed', role: 'producer', agents: [agent('a2', 'producer', 'done', 1500, 1200, 8, '审查文档质量和准确性', '8 个文档有过时信息，3 个缺少代码示例。', '审查文档质量', 6)] },
      { phase_id: 3, label: '生成补全', status: 'completed', role: 'producer', agents: [agent('a3', 'producer', 'done', 2000, 1500, 10, '为缺失内容生成文档', '生成 12 个文档补丁。', '生成文档补全', 7)] },
      { phase_id: 4, label: '验证', status: 'completed', role: 'voter', agents: [agent('a4', 'voter', 'done', 800, 500, 5, '验证补全的文档', '所有补丁质量合格。', '验证文档补丁质量', 2)] },
    ],
    findings: [],
  },
}

// ── Events ──

export function mockEventsForRun(runId: string): AgentEvent[] {
  const cp = mockCheckpoints[runId]
  if (!cp) return []

  const events: AgentEvent[] = []
  events.push({ type: 'RunStarted', run_id: runId, task: cp.task, ts: cp.started_at })

  for (const phase of cp.phases) {
    if (phase.status === 'pending') break
    events.push({
      type: 'PhaseStarted', run_id: runId, phase_id: phase.phase_id,
      label: phase.label, planned: phase.agents.length, ts: cp.started_at,
    })
    for (const a of phase.agents) {
      events.push({
        type: 'AgentStarted', run_id: runId, phase_id: phase.phase_id,
        agent_id: a.agent_id, role: a.role, prompt_preview: a.prompt_preview,
        ts: cp.started_at,
      })
      if (a.status === 'done' || a.status === 'failed') {
        events.push({
          type: 'AgentDone', run_id: runId, agent_id: a.agent_id,
          status: a.status, tokens: a.tokens, elapsed_ms: a.elapsed_ms, ts: cp.started_at,
        })
      }
    }
    const okCount = phase.agents.filter(a => a.status === 'done').length
    const failCount = phase.agents.filter(a => a.status === 'failed').length
    if (phase.status === 'completed') {
      events.push({
        type: 'PhaseDone', run_id: runId, phase_id: phase.phase_id,
        ok: okCount, failed: failCount, ts: cp.started_at,
      })
    }
  }

  if (cp.status === 'completed' || cp.status === 'failed') {
    events.push({
      type: 'RunDone', run_id: runId, status: cp.status,
      total_tokens: cp.total_tokens_detail, ts: cp.started_at,
    })
  }

  return events
}

// ── Dashboard Stats ──

export const mockDashboardStats: DashboardStats = {
  today_runs: 23,
  today_tokens: 142000,
  today_success: 14,
  today_failed: 3,
  active_runs: mockRuns.filter(r => r.status === 'running'),
  recent_runs: mockRuns.filter(r => r.status !== 'running').slice(0, 6),
}

// ── Workflows ──

export const mockWorkflows: WorkflowSummary[] = [
  { name: 'code-review', description: '代码质量分析编排', phases: 3, agents: 5 },
  { name: 'security-audit', description: '安全漏洞扫描与验证', phases: 4, agents: 7 },
  { name: 'test-gen', description: '自动生成测试用例', phases: 3, agents: 4 },
  { name: 'doc-review', description: '文档审查与补全', phases: 4, agents: 4 },
  { name: 'deploy-check', description: '部署前预检查', phases: 5, agents: 8 },
]

export const mockWorkflowDetails: Record<string, WorkflowDetail> = {
  'code-review': {
    name: 'code-review',
    description: '代码质量分析编排',
    content: `-- Phase 1: 生成分析
local phase1 = phase({
  label = "生成分析",
  planned = 3,
})

agent({
  role = "producer",
  prompt = "分析 src/core/ 模块的代码质量，输出结构化的发现。",
})

agent({
  role = "producer",
  prompt = "分析 src/runtime/ 模块的代码质量，输出结构化的发现。",
})

agent({
  role = "producer",
  prompt = "分析 src/cli.rs 模块的代码质量，输出结构化的发现。",
})

-- Phase 2: 对抗验证
local phase2 = phase({
  label = "对抗验证",
  planned = 2,
})

agent({
  role = "producer",
  prompt = "验证 Phase 1 的分析结果，补充遗漏。",
})

agent({
  role = "adversary",
  prompt = "挑战 Phase 1 的分析结论，找出不准确或不完整的部分。",
})

-- Phase 3: 投票收敛
local phase3 = phase({
  label = "投票收敛",
  planned = 1,
})

agent({
  role = "voter",
  prompt = "综合所有分析和对抗结果，产出最终报告。",
})`,
  },
  'security-audit': {
    name: 'security-audit',
    description: '安全漏洞扫描与验证',
    content: `-- Phase 1: 依赖扫描
phase({ label = "依赖扫描", planned = 2 })
agent({ role = "producer", prompt = "扫描 Cargo.toml 中的已知漏洞依赖" })
agent({ role = "producer", prompt = "扫描 npm 中的已知漏洞依赖" })

-- Phase 2: 代码审计
phase({ label = "代码审计", planned = 3 })
agent({ role = "producer", prompt = "检查 SQL 注入风险" })
agent({ role = "producer", prompt = "检查 XSS 和 CSRF 风险" })
agent({ role = "producer", prompt = "检查路径遍历风险" })

-- Phase 3: 对抗验证
phase({ label = "对抗验证", planned = 1 })
agent({ role = "adversary", prompt = "尝试绕过发现的安全防护" })

-- Phase 4: 报告
phase({ label = "报告生成", planned = 1 })
agent({ role = "voter", prompt = "生成安全审计报告" })`,
  },
}

// ── Backends ──

export const mockBackends: BackendConfig[] = [
  { id: 'b1', name: 'Claude Sonnet 4', provider: 'anthropic', model: 'claude-sonnet-4-20250514', connected: true, usage_count: 18 },
  { id: 'b2', name: 'GPT-4o', provider: 'openai', model: 'gpt-4o', connected: true, usage_count: 5 },
  { id: 'b3', name: 'Local Llama', provider: 'ollama', model: 'llama3-70b', connected: false, usage_count: 0 },
]
