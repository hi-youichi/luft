import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'
import { Search, CornerDownLeft } from 'lucide-react'
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from '@/components/ui/dialog'
import { useUIStore } from '@/stores/ui'
import { searchRoutes, type RouteConfig } from '@/lib/routes'
import { cn } from '@/lib/utils'

export function CommandPalette() {
  const open = useUIStore((s) => s.commandPaletteOpen)
  const setOpen = useUIStore((s) => s.setCommandPaletteOpen)
  const navigate = useNavigate()
  const [query, setQuery] = useState('')
  const [activeIndex, setActiveIndex] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)

  const results = useMemo(() => searchRoutes(query), [query])

  const selectRoute = useCallback(
    (route: RouteConfig) => {
      navigate(route.path)
      setOpen(false)
      setQuery('')
      setActiveIndex(0)
    },
    [navigate, setOpen],
  )

  useEffect(() => {
    if (open) {
      setQuery('')
      setActiveIndex(0)
      requestAnimationFrame(() => inputRef.current?.focus())
    }
  }, [open])

  useEffect(() => {
    setActiveIndex(0)
  }, [query])

  useEffect(() => {
    if (!open) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'ArrowDown') {
        e.preventDefault()
        setActiveIndex((i) => (i + 1) % results.length)
      } else if (e.key === 'ArrowUp') {
        e.preventDefault()
        setActiveIndex((i) => (i - 1 + results.length) % results.length)
      } else if (e.key === 'Enter' && results[activeIndex]) {
        e.preventDefault()
        selectRoute(results[activeIndex])
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [open, results, activeIndex, selectRoute])

  useEffect(() => {
    const el = listRef.current?.querySelector(`[data-idx="${activeIndex}"]`)
    el?.scrollIntoView({ block: 'nearest' })
  }, [activeIndex])

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogContent className="top-[20%] max-w-md translate-y-0 p-0 gap-0">
        <DialogHeader className="sr-only">
          <DialogTitle>Navigation</DialogTitle>
          <DialogDescription>Search and navigate to pages</DialogDescription>
        </DialogHeader>

        <div className="flex items-center gap-3 border-b border-border px-4 py-3">
          <Search className="h-4 w-4 text-muted-foreground" />
          <input
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索页面..."
            className="flex-1 bg-transparent text-sm text-foreground outline-none placeholder:text-muted-foreground"
          />
          <kbd className="rounded border border-border px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground">
            ESC
          </kbd>
        </div>

        <div ref={listRef} className="max-h-80 overflow-y-auto p-2">
          {results.length === 0 ? (
            <div className="py-8 text-center text-sm text-muted-foreground">
              未找到匹配的页面
            </div>
          ) : (
            results.map((route, i) => {
              const Icon = route.icon
              return (
                <button
                  key={route.path}
                  data-idx={i}
                  onClick={() => selectRoute(route)}
                  onMouseEnter={() => setActiveIndex(i)}
                  className={cn(
                    'flex w-full items-center gap-3 rounded-md px-3 py-2.5 text-left transition-colors',
                    i === activeIndex ? 'bg-hover' : 'hover:bg-hover/50',
                  )}
                >
                  <Icon className={cn(
                    'h-4 w-4 shrink-0',
                    i === activeIndex ? 'text-primary' : 'text-muted-foreground',
                  )} />
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-medium text-foreground">{route.label}</div>
                    <div className="text-xs text-muted-foreground truncate">{route.description}</div>
                  </div>
                  {i === activeIndex && (
                    <CornerDownLeft className="h-3.5 w-3.5 text-muted-foreground" />
                  )}
                </button>
              )
            })
          )}
        </div>

        <div className="flex items-center justify-between border-t border-border px-4 py-2 text-[11px] text-muted-foreground">
          <div className="flex items-center gap-3">
            <span className="flex items-center gap-1">
              <kbd className="rounded border border-border px-1 font-mono">↑↓</kbd>
              导航
            </span>
            <span className="flex items-center gap-1">
              <kbd className="rounded border border-border px-1 font-mono">↵</kbd>
              选择
            </span>
          </div>
          <span className="font-mono">{results.length} results</span>
        </div>
      </DialogContent>
    </Dialog>
  )
}
