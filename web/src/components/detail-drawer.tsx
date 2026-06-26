import { useState, useEffect } from 'react'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { StatusBadge } from '@/components/status-badge'
import { Tabs, TabsList, TabsTrigger, TabsContent } from '@/components/ui/tabs'
import { formatTokens, formatElapsed } from '@/lib/format'
import type { AgentResultCache } from '@/api/types'

interface DetailDrawerProps {
  agent: AgentResultCache | null
  open: boolean
  onOpenChange: (open: boolean) => void
}

export function DetailDrawer({ agent, open, onOpenChange }: DetailDrawerProps) {
  const [tab, setTab] = useState('overview')

  useEffect(() => {
    if (open) setTab('overview')
  }, [open, agent?.agent_id])

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent side="right" className="w-full sm:max-w-md flex flex-col p-0">
        {agent && (
          <>
            <SheetHeader className="px-6 pt-6 pb-3 border-b border-border">
              <SheetTitle>{agent.agent_id}</SheetTitle>
              <div className="flex items-center gap-2 mt-1">
                <StatusBadge status={agent.status} />
                <span className="text-xs text-muted-foreground capitalize">{agent.role}</span>
              </div>
            </SheetHeader>

            <Tabs value={tab} onValueChange={setTab} className="flex-1 flex flex-col overflow-hidden">
              <div className="px-6 pt-3 border-b border-border">
                <TabsList className="bg-transparent p-0 h-auto gap-4">
                  <TabsTrigger value="overview" className="rounded-none border-b-2 border-transparent data-[state=active]:border-primary data-[state=active]:bg-transparent data-[state=active]:shadow-none px-0 pb-2">Overview</TabsTrigger>
                  <TabsTrigger value="prompt" className="rounded-none border-b-2 border-transparent data-[state=active]:border-primary data-[state=active]:bg-transparent data-[state=active]:shadow-none px-0 pb-2">Prompt</TabsTrigger>
                  <TabsTrigger value="output" className="rounded-none border-b-2 border-transparent data-[state=active]:border-primary data-[state=active]:bg-transparent data-[state=active]:shadow-none px-0 pb-2">Output</TabsTrigger>
                </TabsList>
              </div>

              <TabsContent value="overview" className="flex-1 overflow-y-auto px-6 py-4 mt-0">
                <div className="grid grid-cols-2 gap-3">
                  <div className="rounded-lg border border-border bg-card p-3">
                    <div className="text-xs text-muted-foreground mb-1">Tokens</div>
                    <div className="font-mono text-sm">
                      {formatTokens(agent.tokens.input + agent.tokens.output)}
                    </div>
                    <div className="text-xs text-muted-foreground">
                      {agent.tokens.input} in / {agent.tokens.output} out
                    </div>
                  </div>
                  <div className="rounded-lg border border-border bg-card p-3">
                    <div className="text-xs text-muted-foreground mb-1">Elapsed</div>
                    <div className="font-mono text-sm">
                      {agent.elapsed_ms > 0 ? formatElapsed(agent.elapsed_ms) : '—'}
                    </div>
                  </div>
                </div>

                {agent.description && (
                  <div className="mt-4">
                    <div className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-2">
                      Description
                    </div>
                    <div className="rounded-lg border border-border bg-card p-3 text-sm text-muted-foreground">
                      {agent.description}
                    </div>
                  </div>
                )}

                {agent.error && (
                  <div className="mt-4">
                    <div className="text-xs font-semibold text-destructive uppercase tracking-wide mb-2">
                      Error
                    </div>
                    <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-sm text-destructive whitespace-pre-wrap">
                      {agent.error}
                    </div>
                  </div>
                )}
              </TabsContent>

              <TabsContent value="prompt" className="flex-1 overflow-y-auto px-6 py-4 mt-0">
                {agent.prompt_preview ? (
                  <div className="rounded-lg border border-border bg-card p-4 text-sm text-muted-foreground whitespace-pre-wrap font-mono leading-relaxed">
                    {agent.prompt_preview}
                  </div>
                ) : (
                  <div className="text-sm text-muted-foreground py-8 text-center">No prompt data</div>
                )}
              </TabsContent>

              <TabsContent value="output" className="flex-1 overflow-y-auto px-6 py-4 mt-0">
                {agent.output_preview ? (
                  <div className="rounded-lg border border-border bg-card p-4 text-sm text-muted-foreground whitespace-pre-wrap font-mono leading-relaxed">
                    {agent.output_preview}
                  </div>
                ) : (
                  <div className="text-sm text-muted-foreground py-8 text-center">No output data</div>
                )}
              </TabsContent>
            </Tabs>
          </>
        )}
      </SheetContent>
    </Sheet>
  )
}
