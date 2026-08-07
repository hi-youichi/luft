export interface WorkflowTemplate {
  id: string
  label: string
  description: string
  content: string
}

export const workflowTemplates: WorkflowTemplate[] = [
  {
    id: 'blank',
    label: 'Blank',
    description: 'Empty workflow with a single phase',
    content: `-- New workflow
phase({ label = "Phase 1", planned = 1 })
agent({ role = "producer", prompt = "" })
`,
  },
  {
    id: 'single-agent',
    label: 'Single Agent',
    description: 'One agent, one phase',
    content: `-- Single-agent workflow
phase({ label = "Execute", planned = 1 })
agent({
  role = "producer",
  prompt = "Describe the task here.",
})
`,
  },
  {
    id: 'code-review',
    label: 'Code Review',
    description: '3-phase: analyze, challenge, converge',
    content: `-- Code review workflow
phase({ label = "Analysis", planned = 3 })
agent({ role = "producer", prompt = "Analyze code quality of the target." })
agent({ role = "producer", prompt = "Check for potential bugs and edge cases." })
agent({ role = "producer", prompt = "Review test coverage and suggest improvements." })

phase({ label = "Adversarial", planned = 1 })
agent({ role = "adversary", prompt = "Challenge the analysis findings and identify gaps." })

phase({ label = "Converge", planned = 1 })
agent({ role = "voter", prompt = "Synthesize all findings into a final report." })
`,
  },
  {
    id: 'pipeline',
    label: 'Pipeline',
    description: 'Streaming multi-stage pipeline',
    content: `-- Pipeline workflow
pipeline({
  items = { "module-a", "module-b", "module-c" },
  stages = {
    function(item)
      agent({ role = "producer", prompt = "Analyze " .. item })
    end,
    function(item)
      agent({ role = "adversary", prompt = "Validate analysis for " .. item })
    end,
  },
  max_inflight = 2,
})
`,
  },
  {
    id: 'parallel',
    label: 'Parallel Fan-out',
    description: 'Parallel processing with convergence',
    content: `-- Parallel fan-out workflow
phase({ label = "Parallel Analysis", planned = 3 })
parallel({ "src/a", "src/b", "src/c" }, function(item)
  agent({ role = "producer", prompt = "Analyze " .. item })
end)

phase({ label = "Converge", planned = 1 })
agent({ role = "voter", prompt = "Merge all parallel results into a summary." })
`,
  },
]
