import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query'
import { useParams, useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
import { api, queryKeys } from '@/api'
import type { WorkflowSummary } from '@/api/types'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Badge } from '@/components/ui/badge'
import { Separator } from '@/components/ui/separator'
import { Skeleton } from '@/components/ui/skeleton'
import { CodeEditor } from '@/components/code-editor'
import {
  FileCode2, Save, Play, Plus, Trash2, Pencil, Eye,
  RefreshCw, AlertCircle, FileText, Layers, Users, ArrowRight, Copy,
} from 'lucide-react'
import { useState, useEffect, useMemo, useCallback } from 'react'
import { cn } from '@/lib/utils'
import { workflowTemplates } from '@/lib/workflow-templates'

interface ParsedPhase {
  label: string
  planned: number
  agents: ParsedAgent[]
}

interface ParsedAgent {
  role: string
  prompt: string
}

function parseLuaWorkflow(content: string): { phases: ParsedPhase[]; totalAgents: number } {
  const phases: ParsedPhase[] = []
  let totalAgents = 0

  const phasePattern = /phase\s*\(\s*\{[^}]*label\s*=\s*"([^"]+)"[^}]*planned\s*=\s*(\d+)[^}]*\}/g
  const agentPattern = /agent\s*\(\s*\{[^}]*role\s*=\s*"([^"]+)"[^}]*prompt\s*=\s*"([^"]*)"/g

  const phaseMatches: { label: string; planned: number; index: number }[] = []
  let pm: RegExpExecArray | null
  while ((pm = phasePattern.exec(content)) !== null) {
    phaseMatches.push({ label: pm[1], planned: parseInt(pm[2], 10), index: pm.index })
  }

  const agentMatches: { role: string; prompt: string; index: number }[] = []
  let am: RegExpExecArray | null
  while ((am = agentPattern.exec(content)) !== null) {
    agentMatches.push({ role: am[1], prompt: am[2], index: am.index })
  }

  if (phaseMatches.length === 0) {
    totalAgents = agentMatches.length
    return { phases: [], totalAgents }
  }

  for (let i = 0; i < phaseMatches.length; i++) {
    const start = phaseMatches[i].index
    const end = i < phaseMatches.length - 1 ? phaseMatches[i + 1].index : content.length
    const agentsInPhase = agentMatches.filter(a => a.index >= start && a.index < end)
    phases.push({
      label: phaseMatches[i].label,
      planned: phaseMatches[i].planned,
      agents: agentsInPhase.map(a => ({ role: a.role, prompt: a.prompt })),
    })
    totalAgents += agentsInPhase.length
  }

  return { phases, totalAgents }
}

function PhaseFlow({ phases }: { phases: ParsedPhase[] }) {
  if (phases.length === 0) {
    return (
      <div className="flex items-center gap-2 px-3 py-4 text-xs text-muted-foreground">
        <AlertCircle className="h-3.5 w-3.5" />
        No phase definitions found in workflow content
      </div>
    )
  }

  const roleColors: Record<string, string> = {
    producer: 'bg-emerald-500/15 text-emerald-400',
    adversary: 'bg-red-500/15 text-red-400',
    voter: 'bg-sky-500/15 text-sky-400',
    default: 'bg-muted text-muted-foreground',
  }

  return (
    <div className="space-y-1">
      {phases.map((phase, i) => (
        <div key={i}>
          <div className="flex items-center gap-2">
            <div className="flex h-6 w-6 shrink-0 items-center justify-center rounded bg-primary/15 text-xs font-bold text-primary">
              {i + 1}
            </div>
            <span className="text-sm font-medium">{phase.label}</span>
            <Badge variant="secondary" className="ml-auto text-[10px]">
              {phase.planned} planned
            </Badge>
          </div>
          {phase.agents.length > 0 && (
            <div className="ml-8 mt-1 space-y-1 border-l border-border pl-3">
              {phase.agents.map((agent, j) => (
                <div key={j} className="flex items-center gap-2">
                  <span className={cn(
                    'rounded px-1.5 py-0.5 text-[10px] font-medium',
                    roleColors[agent.role] ?? roleColors.default,
                  )}>
                    {agent.role}
                  </span>
                  <span className="text-xs text-muted-foreground truncate max-w-md">
                    {agent.prompt}
                  </span>
                </div>
              ))}
            </div>
          )}
          {i < phases.length - 1 && (
            <div className="ml-3 h-3 border-l border-border" />
          )}
        </div>
      ))}
    </div>
  )
}

export function WorkflowsPage() {
  const params = useParams<{ workflowName?: string }>()
  const navigate = useNavigate()
  const queryClient = useQueryClient()

  const [selectedName, setSelectedName] = useState<string | null>(params.workflowName ?? null)
  const [editMode, setEditMode] = useState(false)
  const [editorContent, setEditorContent] = useState('')
  const [searchQuery, setSearchQuery] = useState('')
  const [showNewDialog, setShowNewDialog] = useState(false)
  const [newWorkflowName, setNewWorkflowName] = useState('')
  const [selectedTemplateId, setSelectedTemplateId] = useState(workflowTemplates[0].id)

  useEffect(() => {
    if (params.workflowName) {
      setSelectedName(params.workflowName)
    }
  }, [params.workflowName])

  const workflowsQuery = useQuery({
    queryKey: queryKeys.workflows.all,
    queryFn: () => api.workflows.list(),
  })

  const detailQuery = useQuery({
    queryKey: queryKeys.workflows.detail(selectedName ?? ''),
    queryFn: () => api.workflows.get(selectedName!),
    enabled: !!selectedName,
  })

  useEffect(() => {
    if (detailQuery.data) {
      setEditorContent(detailQuery.data.content)
    }
  }, [detailQuery.data])

  const hasChanges = detailQuery.data && editorContent !== detailQuery.data.content

  const handleSelect = useCallback((name: string) => {
    setSelectedName(name)
    setEditMode(false)
    navigate(`/workflows/${name}`, { replace: true })
  }, [navigate])

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (!selectedName || !editMode) return
      if ((e.metaKey || e.ctrlKey) && e.key === 's') {
        e.preventDefault()
        if (hasChanges && !saveMutation.isPending) {
          handleSave()
        }
      }
      if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
        e.preventDefault()
        if (!runMutation.isPending) {
          handleRun()
        }
      }
    }
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [selectedName, editMode, editorContent, hasChanges])

  const saveMutation = useMutation({
    mutationFn: (req: { name: string; content: string }) =>
      api.workflows.save({ name: req.name, content: req.content }),
    onSuccess: () => {
      toast.success('Workflow saved')
      queryClient.invalidateQueries({ queryKey: queryKeys.workflows.all })
      if (selectedName) {
        queryClient.invalidateQueries({ queryKey: queryKeys.workflows.detail(selectedName) })
      }
      setEditMode(false)
    },
    onError: () => toast.error('Failed to save workflow'),
  })

  const deleteMutation = useMutation({
    mutationFn: (name: string) => api.workflows.delete(name),
    onSuccess: () => {
      toast.success('Workflow deleted')
      queryClient.invalidateQueries({ queryKey: queryKeys.workflows.all })
      setSelectedName(null)
      navigate('/workflows', { replace: true })
    },
    onError: () => toast.error('Failed to delete workflow'),
  })

  const runMutation = useMutation({
    mutationFn: (name: string) => api.workflows.run({ name }),
    onSuccess: (res) => {
      toast.success('Run started', { description: res.run_id })
      navigate(`/runs/${res.run_id}`)
    },
    onError: () => toast.error('Failed to start run'),
  })

  const handleSave = () => {
    if (!selectedName) return
    saveMutation.mutate({ name: selectedName, content: editorContent })
  }

  const handleDelete = () => {
    if (!selectedName) return
    if (confirm(`Delete workflow "${selectedName}"? This cannot be undone.`)) {
      deleteMutation.mutate(selectedName)
    }
  }

  const handleRun = () => {
    if (!selectedName) return
    runMutation.mutate(selectedName)
  }

  const handleDuplicate = () => {
    if (!selectedName) return
    const dupName = `${selectedName}-copy`
    api.workflows.save({ name: dupName, content: editorContent })
      .then(() => {
        toast.success(`Duplicated as "${dupName}"`)
        queryClient.invalidateQueries({ queryKey: queryKeys.workflows.all })
        handleSelect(dupName)
      })
      .catch(() => toast.error('Failed to duplicate workflow'))
  }

  const handleCreateNew = () => {
    const name = newWorkflowName.trim().replace(/\s+/g, '-')
    if (!name) return
    const template = workflowTemplates.find(t => t.id === selectedTemplateId) ?? workflowTemplates[0]
    api.workflows.save({ name, content: template.content })
      .then(() => {
        toast.success('Workflow created')
        queryClient.invalidateQueries({ queryKey: queryKeys.workflows.all })
        setShowNewDialog(false)
        setNewWorkflowName('')
        handleSelect(name)
      })
      .catch(() => toast.error('Failed to create workflow'))
  }

  const parsed = useMemo(() => {
    if (!editorContent) return { phases: [] as ParsedPhase[], totalAgents: 0 }
    return parseLuaWorkflow(editorContent)
  }, [editorContent])

  const filteredWorkflows = useMemo(() => {
    if (!workflowsQuery.data) return []
    if (!searchQuery) return workflowsQuery.data
    const q = searchQuery.toLowerCase()
    return workflowsQuery.data.filter(
      (w) => w.name.toLowerCase().includes(q) || w.description.toLowerCase().includes(q),
    )
  }, [workflowsQuery.data, searchQuery])

  return (
    <div className="flex h-[calc(100vh-3.5rem)] flex-col">
      <div className="flex flex-1 overflow-hidden">
        {/* Left: File Browser */}
        <div className="w-64 shrink-0 border-r border-border bg-bg-surface/40">
          <div className="flex items-center justify-between px-3 py-2.5">
            <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
              Workflows
            </span>
            <Button
              variant="ghost"
              size="sm"
              className="h-6 w-6 p-0"
              onClick={() => setShowNewDialog(!showNewDialog)}
            >
              <Plus className="h-3.5 w-3.5" />
            </Button>
          </div>

          {showNewDialog && (
            <div className="px-3 pb-2 space-y-2">
              <Input
                placeholder="workflow-name"
                value={newWorkflowName}
                onChange={(e) => setNewWorkflowName(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && handleCreateNew()}
                className="h-7 text-xs"
                autoFocus
              />
              <div className="space-y-1">
                <span className="text-[10px] text-muted-foreground uppercase tracking-wider">Template</span>
                <div className="grid grid-cols-1 gap-0.5">
                  {workflowTemplates.map(tpl => (
                    <button
                      key={tpl.id}
                      onClick={() => setSelectedTemplateId(tpl.id)}
                      className={cn(
                        'flex items-start gap-1.5 rounded px-2 py-1 text-left transition-colors',
                        selectedTemplateId === tpl.id
                          ? 'bg-primary/15'
                          : 'hover:bg-hover/50',
                      )}
                    >
                      <span className={cn(
                        'mt-0.5 h-2 w-2 shrink-0 rounded-full',
                        selectedTemplateId === tpl.id ? 'bg-primary' : 'bg-muted-foreground/30',
                      )} />
                      <div>
                        <span className="text-[11px] font-medium">{tpl.label}</span>
                        <span className="block text-[9px] text-muted-foreground">{tpl.description}</span>
                      </div>
                    </button>
                  ))}
                </div>
              </div>
              <div className="flex gap-1">
                <Button size="sm" className="h-6 text-xs flex-1" onClick={handleCreateNew}>
                  Create
                </Button>
                <Button variant="ghost" size="sm" className="h-6 text-xs" onClick={() => setShowNewDialog(false)}>
                  Cancel
                </Button>
              </div>
            </div>
          )}

          <div className="px-3 pb-2">
            <Input
              placeholder="Search workflows..."
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              className="h-7 text-xs"
            />
          </div>

          <div className="overflow-y-auto px-1.5 pb-2" style={{ maxHeight: 'calc(100vh - 12rem)' }}>
            {workflowsQuery.isLoading ? (
              Array.from({ length: 4 }).map((_, i) => (
                <Skeleton key={i} className="mb-1 h-12" />
              ))
            ) : filteredWorkflows.length === 0 ? (
              <div className="px-2 py-4 text-center text-xs text-muted-foreground">
                No workflows found
              </div>
            ) : (
              filteredWorkflows.map((wf) => (
                <WorkflowFileItem
                  key={wf.name}
                  workflow={wf}
                  selected={wf.name === selectedName}
                  onClick={() => handleSelect(wf.name)}
                />
              ))
            )}
          </div>
        </div>

        {/* Right: Editor + Schema Preview */}
        {!selectedName ? (
          <EmptyState />
        ) : detailQuery.isLoading ? (
          <div className="flex-1 p-6">
            <Skeleton className="mb-4 h-10 w-full" />
            <Skeleton className="h-[60%] w-full" />
          </div>
        ) : detailQuery.error ? (
          <div className="flex flex-1 flex-col items-center justify-center gap-3 text-center">
            <AlertCircle className="h-8 w-8 text-destructive" />
            <p className="text-sm text-muted-foreground">
              Failed to load workflow: {detailQuery.error.message}
            </p>
            <Button
              variant="outline"
              size="sm"
              onClick={() => detailQuery.refetch()}
            >
              <RefreshCw className="h-3.5 w-3.5" />
              Retry
            </Button>
          </div>
        ) : (
          <div className="flex flex-1 flex-col overflow-hidden">
            {/* Toolbar */}
            <div className="flex items-center justify-between border-b border-border px-4 py-2">
              <div className="flex items-center gap-2">
                <FileCode2 className="h-4 w-4 text-muted-foreground" />
                <span className="text-sm font-medium font-mono">{selectedName}</span>
                {detailQuery.data?.last_modified && (
                  <span className="text-xs text-muted-foreground">
                    · modified {new Date(detailQuery.data.last_modified).toLocaleDateString('zh-CN')}
                  </span>
                )}
                {hasChanges && (
                  <Badge className="ml-1 text-[10px]">unsaved</Badge>
                )}
              </div>
              <div className="flex items-center gap-1">
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() => {
                    if (editMode && hasChanges) {
                      setEditorContent(detailQuery.data?.content ?? '')
                    }
                    setEditMode(!editMode)
                  }}
                >
                  {editMode ? <Eye className="h-3.5 w-3.5" /> : <Pencil className="h-3.5 w-3.5" />}
                  {editMode ? 'View' : 'Edit'}
                </Button>
                <Separator orientation="vertical" className="mx-1 h-5" />
                <Button
                  variant="outline"
                  size="sm"
                  onClick={handleRun}
                  disabled={runMutation.isPending}
                >
                  <Play className="h-3.5 w-3.5" />
                  Run
                </Button>
                {editMode && (
                  <Button
                    size="sm"
                    onClick={handleSave}
                    disabled={saveMutation.isPending || !hasChanges}
                  >
                    <Save className="h-3.5 w-3.5" />
                    Save
                  </Button>
                )}
                <Separator orientation="vertical" className="mx-1 h-5" />
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={handleDuplicate}
                  title="Duplicate workflow"
                >
                  <Copy className="h-3.5 w-3.5" />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  className="text-destructive hover:text-destructive"
                  onClick={handleDelete}
                  disabled={deleteMutation.isPending}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            </div>

            {/* Editor + Schema Split */}
            <div className="flex flex-1 overflow-hidden">
              {/* Code Editor */}
              <div className="flex-1 overflow-hidden">
                <CodeEditor
                  value={editorContent}
                  onChange={editMode ? setEditorContent : undefined}
                  readOnly={!editMode}
                  className="h-full"
                />
              </div>

              {/* Schema Preview Sidebar */}
              <div className="w-72 shrink-0 overflow-y-auto border-l border-border bg-bg-surface/40">
                <div className="px-4 py-3">
                  <div className="mb-3 flex items-center gap-2">
                    <Layers className="h-3.5 w-3.5 text-muted-foreground" />
                    <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                      Schema
                    </span>
                  </div>

                  {/* Stats */}
                  <div className="mb-4 grid grid-cols-2 gap-2">
                    <div className="rounded-md bg-muted/50 p-2">
                      <div className="flex items-center gap-1 text-[10px] text-muted-foreground">
                        <Layers className="h-3 w-3" />
                        Phases
                      </div>
                      <div className="text-lg font-bold">{parsed.phases.length}</div>
                    </div>
                    <div className="rounded-md bg-muted/50 p-2">
                      <div className="flex items-center gap-1 text-[10px] text-muted-foreground">
                        <Users className="h-3 w-3" />
                        Agents
                      </div>
                      <div className="text-lg font-bold">{parsed.totalAgents}</div>
                    </div>
                  </div>

                  <Separator className="mb-3" />

                  {/* Phase Flow */}
                  <PhaseFlow phases={parsed.phases} />

                  {parsed.phases.length > 0 && (
                    <>
                      <Separator className="my-3" />
                      <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                        <ArrowRight className="h-3 w-3" />
                        Sequential execution
                      </div>
                    </>
                  )}
                </div>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  )
}

function WorkflowFileItem({
  workflow,
  selected,
  onClick,
}: {
  workflow: WorkflowSummary
  selected: boolean
  onClick: () => void
}) {
  return (
    <button
      onClick={onClick}
      className={cn(
        'mb-0.5 flex w-full flex-col items-start rounded-md px-2.5 py-2 text-left transition-colors',
        selected
          ? 'bg-hover'
          : 'hover:bg-hover/50',
      )}
    >
      <div className="flex items-center gap-1.5">
        <FileCode2 className={cn(
          'h-3 w-3 shrink-0',
          selected ? 'text-primary' : 'text-muted-foreground',
        )} />
        <span className={cn(
          'text-xs font-medium font-mono',
          selected ? 'text-primary' : 'text-foreground',
        )}>
          {workflow.name}
        </span>
      </div>
      <span className="mt-0.5 pl-4.5 text-[10px] text-muted-foreground line-clamp-1">
        {workflow.description}
      </span>
      <div className="mt-1 flex items-center gap-2 pl-4.5">
        <span className="text-[9px] text-muted-foreground">
          {workflow.phases}p · {workflow.agents}a
        </span>
      </div>
    </button>
  )
}

function EmptyState() {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-4 text-center">
      <div className="flex h-16 w-16 items-center justify-center rounded-2xl bg-muted/50">
        <FileText className="h-7 w-7 text-muted-foreground" />
      </div>
      <div>
        <p className="text-sm font-medium">No workflow selected</p>
        <p className="mt-1 text-xs text-muted-foreground">
          Select a workflow from the list to view or edit
        </p>
      </div>
    </div>
  )
}

export default WorkflowsPage
