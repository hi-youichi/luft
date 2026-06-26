import { useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { toast } from 'sonner'
import { Dialog, DialogContent, DialogHeader, DialogTitle, DialogFooter } from '@/components/ui/dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Select, SelectTrigger, SelectValue, SelectContent, SelectItem } from '@/components/ui/select'
import { useUIStore } from '@/stores/ui'
import { mockApi } from '@/api/mock-client'
import { mockWorkflows, mockBackends } from '@/api/mock-data'

export function RunDialog() {
  const open = useUIStore((s) => s.runDialogOpen)
  const setOpen = useUIStore((s) => s.setRunDialogOpen)
  const navigate = useNavigate()
  const [workflow, setWorkflow] = useState('code-review')
  const [task, setTask] = useState('')
  const [backend, setBackend] = useState('b1')
  const [loading, setLoading] = useState(false)

  async function handleStart() {
    if (!task.trim()) return
    setLoading(true)
    try {
      const res = await mockApi.runs.start({ workflow, task, backend })
      toast.success('Run started', { description: res.run_id })
      setOpen(false)
      navigate(`/runs/${res.run_id}`)
    } catch {
      toast.error('Failed to start run')
    } finally {
      setLoading(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>发起 Run</DialogTitle>
        </DialogHeader>
        <div className="space-y-4 py-2">
          <div className="space-y-1.5">
            <Label>Workflow</Label>
            <Select value={workflow} onValueChange={setWorkflow}>
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent>
                {mockWorkflows.map((w) => (
                  <SelectItem key={w.name} value={w.name}>{w.name}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1.5">
            <Label>Task 描述</Label>
            <Input
              placeholder="分析 src/ 目录的代码质量..."
              value={task}
              onChange={(e) => setTask(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && handleStart()}
            />
          </div>
          <div className="space-y-1.5">
            <Label>Backend</Label>
            <Select value={backend} onValueChange={setBackend}>
              <SelectTrigger><SelectValue /></SelectTrigger>
              <SelectContent>
                {mockBackends.map((b) => (
                  <SelectItem key={b.id} value={b.id}>{b.name}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => setOpen(false)}>取消</Button>
          <Button onClick={handleStart} disabled={!task.trim() || loading}>
            {loading ? 'Starting...' : '开始 Run'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
