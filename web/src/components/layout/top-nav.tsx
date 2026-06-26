import { NavLink } from 'react-router-dom'
import { Activity, ListChecks, FileCode2, Server, Play } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useUIStore } from '@/stores/ui'
import { cn } from '@/lib/utils'

const navItems = [
  { to: '/', label: 'Dashboard', icon: Activity },
  { to: '/runs', label: 'Runs', icon: ListChecks },
  { to: '/workflows', label: 'Workflows', icon: FileCode2 },
  { to: '/backends', label: 'Backends', icon: Server },
]

export function TopNav() {
  const setRunDialogOpen = useUIStore((s) => s.setRunDialogOpen)

  return (
    <header className="sticky top-0 z-40 flex h-14 items-center justify-between border-b border-border bg-bg-surface/80 px-6 backdrop-blur-md">
      <div className="flex items-center gap-2">
        <div className="flex items-center gap-2 mr-8">
          <div className="flex h-7 w-7 items-center justify-center rounded-md bg-primary/15">
            <span className="text-primary text-base font-bold font-display">M</span>
          </div>
          <span className="text-base font-semibold font-display tracking-tight">maestro</span>
        </div>
        <nav className="flex items-center gap-1">
          {navItems.map(({ to, label, icon: Icon }) => (
            <NavLink
              key={to}
              to={to}
              end={to === '/'}
              className={({ isActive }) =>
                cn(
                  'flex items-center gap-2 rounded-md px-3 py-1.5 text-sm font-medium transition-colors',
                  isActive
                    ? 'bg-hover text-primary'
                    : 'text-muted-foreground hover:text-foreground hover:bg-hover/50'
                )
              }
            >
              <Icon className="h-4 w-4" />
              {label}
            </NavLink>
          ))}
        </nav>
      </div>
      <Button size="sm" onClick={() => setRunDialogOpen(true)}>
        <Play className="h-3.5 w-3.5" />
        Run
      </Button>
    </header>
  )
}
