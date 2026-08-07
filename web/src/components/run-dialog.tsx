import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { useQuery, useMutation } from '@tanstack/react-query'
import { toast } from 'sonner'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import { useUIStore } from '@/stores/ui'
import { api, queryKeys } from '@/api'

export function RunDialog() {
  const open = useUIStore((s) => s.runDialogOpen)
  const setOpen = useUIStore((s) => s.setRunDialogOpen)
  const navigate = useNavigate()
  const [workflow, setWorkflow] = useState('')
  const [task, setTask] = useState('')
  const [backend, setBackend] = useState('')

  const workflowsQuery = useQuery({
    queryKey: queryKeys.workflows.all,
    queryFn: () => api.workflows.list(),
    enabled: open,
  })

  const backendsQuery = useQuery({
    queryKey: queryKeys.backends.all,
    queryFn: () => api.backends.list(),
    enabled: open,
  })

  const startMutation = useMutation({
    mutationFn: () =>
      api.runs.start({ workflow, task: task.trim(), backend }),
    onSuccess: (res) => {
      toast.success('Run started', { description: res.run_id })
      setOpen(false)
      setTask('')
      navigate(`/runs/${res.run_id}`)
    },
    onError: () => toast.error('Failed to start run'),
  })

  const connectedBackends = backendsQuery.data?.filter((b) => b.connected) ?? []

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>发起 Run</DialogTitle>
        </DialogHeader>
        <div className="space-y-4 py-2">
          <div className="space-y-1.5">
            <Label>Workflow</Label>
            {workflowsQuery.isLoading ? (
              <Skeleton className="h-9 w-full" />
            ) : workflowsQuery.data && workflowsQuery.data.length > 0 ? (
              <Select
                value={workflow || undefined}
                onValueChange={setWorkflow}
              >
                <SelectTrigger><SelectValue placeholder="选择 workflow..." /></SelectTrigger>
                <SelectContent>
                  {workflowsQuery.data.map((w) => (
                    <SelectItem key={w.name} value={w.name}>
                      {w.name}
                      <span className="ml-2 text-xs text-muted-foreground">— {w.description}</span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : (
              <p className="text-sm text-muted-foreground">No workflows available</p>
            )}
          </div>
          <div className="space-y-1.5">
            <Label>Task 描述</Label>
            <Input
              placeholder="分析 src/ 目录的代码质量..."
              value={task}
              onChange={(e) => setTask(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && startMutation.mutate()}
            />
          </div>
          <div className="space-y-1.5">
            <Label>Backend</Label>
            {backendsQuery.isLoading ? (
              <Skeleton className="h-9 w-full" />
            ) : connectedBackends.length > 0 ? (
              <Select
                value={backend || undefined}
                onValueChange={setBackend}
              >
                <SelectTrigger><SelectValue placeholder="选择 backend..." /></SelectTrigger>
                <SelectContent>
                  {connectedBackends.map((b) => (
                    <SelectItem key={b.id} value={b.id}>
                      {b.name}
                      <span className="ml-2 text-xs text-muted-foreground">({b.model})</span>
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            ) : (
              <p className="text-sm text-muted-foreground">No connected backends</p>
            )}
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => setOpen(false)}>取消</Button>
          <Button
            onClick={() => startMutation.mutate()}
            disabled={!task.trim() || !workflow || !backend || startMutation.isPending}
          >
            {startMutation.isPending ? 'Starting...' : '开始 Run'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
