import { useState, useMemo } from 'react'
import { useQuery } from '@tanstack/react-query'
import { FileCode2, Plus, Save } from 'lucide-react'
import { mockApi } from '@/api/mock-client'
import { mockWorkflows } from '@/api/mock-data'
import { Card } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { CodeEditor } from '@/components/code-editor'
import { cn } from '@/lib/utils'
import { toast } from 'sonner'

export function WorkflowsPage() {
  const [selected, setSelected] = useState('code-review')
  const [content, setContent] = useState('')
  const { data: detail, isLoading } = useQuery({
    queryKey: ['workflow', selected],
    queryFn: () => mockApi.workflows.get(selected),
  })

  useMemo(() => {
    if (detail) setContent(detail.content)
  }, [detail])

  return (
    <div className="space-y-4">
      <h1 className="text-xl font-semibold font-display">Workflows</h1>

      <div className="flex gap-4">
        {/* List */}
        <div className="w-60 shrink-0 space-y-1">
          <Button variant="outline" size="sm" className="w-full justify-start mb-2" onClick={() => toast.info('TODO: 新建 Workflow')}>
            <Plus className="h-3.5 w-3.5" /> 新建
          </Button>
          {mockWorkflows.map((wf) => (
            <button
              key={wf.name}
              onClick={() => setSelected(wf.name)}
              className={cn(
                'flex w-full items-start gap-2 rounded-md p-2.5 text-left transition-colors',
                selected === wf.name ? 'bg-hover text-foreground' : 'text-muted-foreground hover:text-foreground hover:bg-hover/50'
              )}
            >
              <FileCode2 className="h-4 w-4 shrink-0 mt-0.5" />
              <div className="min-w-0">
                <div className="text-sm font-medium truncate">{wf.name}</div>
                <div className="text-xs text-muted-foreground truncate">{wf.description}</div>
              </div>
            </button>
          ))}
        </div>

        {/* Editor */}
        <div className="flex-1 min-w-0">
          {isLoading || !detail ? (
            <Skeleton className="h-96" />
          ) : (
            <Card className="overflow-hidden">
              <div className="flex items-center justify-between px-4 py-2.5 border-b border-border">
                <div>
                  <span className="text-sm font-medium font-mono">{detail.name}.lua</span>
                  <span className="ml-3 text-xs text-muted-foreground">{detail.description}</span>
                </div>
                <Button size="sm" onClick={() => toast.success('已保存')}>
                  <Save className="h-3.5 w-3.5" /> 保存
                </Button>
              </div>
              <div className="h-[520px] overflow-hidden [&_.cm-editor]:h-full [&_.cm-scroller]:overflow-auto">
                <CodeEditor value={content} onChange={setContent} />
              </div>
            </Card>
          )}
        </div>
      </div>
    </div>
  )
}
