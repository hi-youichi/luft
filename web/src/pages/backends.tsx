import { useQuery } from '@tanstack/react-query'
import { mockApi } from '@/api/mock-client'
import { Card } from '@/components/ui/card'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Server, Check, X } from 'lucide-react'
import { cn } from '@/lib/utils'

export function BackendsPage() {
  const { data: backends, isLoading } = useQuery({
    queryKey: ['backends'],
    queryFn: () => mockApi.backends.list(),
  })

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold font-display">Backends</h1>
        <Button size="sm">+ 添加</Button>
      </div>

      {isLoading || !backends ? (
        <div className="grid grid-cols-3 gap-4">
          {Array.from({ length: 3 }).map((_, i) => <Skeleton key={i} className="h-40" />)}
        </div>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-4">
          {backends.map((b) => (
            <Card key={b.id} className="p-5">
              <div className="flex items-start justify-between mb-3">
                <div className="flex items-center gap-2">
                  <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-hover">
                    <Server className="h-4 w-4 text-muted-foreground" />
                  </div>
                  <div>
                    <div className="text-sm font-medium">{b.name}</div>
                    <div className="text-xs text-muted-foreground">{b.provider}</div>
                  </div>
                </div>
                <div className={cn(
                  'flex items-center gap-1 rounded px-1.5 py-0.5 text-xs',
                  b.connected ? 'bg-primary/12 text-primary' : 'bg-muted text-muted-foreground'
                )}>
                  {b.connected ? <Check className="h-3 w-3" /> : <X className="h-3 w-3" />}
                  {b.connected ? '已连接' : '未连接'}
                </div>
              </div>
              <div className="space-y-1 text-xs text-muted-foreground">
                <div>Model: <span className="font-mono text-muted-foreground">{b.model}</span></div>
                <div>使用次数: <span className="font-mono text-muted-foreground">{b.usage_count}</span></div>
              </div>
              <div className="mt-4 flex gap-2">
                <Button variant="outline" size="sm">测试</Button>
                <Button variant="ghost" size="sm">编辑</Button>
              </div>
            </Card>
          ))}
        </div>
      )}
    </div>
  )
}
