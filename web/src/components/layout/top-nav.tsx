import { NavLink } from 'react-router-dom'
import { Play, Search } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useUIStore } from '@/stores/ui'
import { navItems } from '@/lib/routes'
import { MobileNav } from '@/components/layout/mobile-nav'
import { cn } from '@/lib/utils'

export function TopNav() {
  const setRunDialogOpen = useUIStore((s) => s.setRunDialogOpen)
  const setCommandPaletteOpen = useUIStore((s) => s.setCommandPaletteOpen)

  return (
    <header className="sticky top-0 z-40 flex h-14 items-center justify-between border-b border-border bg-bg-surface/80 px-4 backdrop-blur-md sm:px-6">
      <div className="flex items-center gap-2">
        <MobileNav />
        <div className="mr-4 flex items-center gap-2 sm:mr-8">
          <div className="flex h-7 w-7 items-center justify-center rounded-md bg-primary/15">
            <span className="text-base font-bold text-primary font-display">M</span>
          </div>
          <span className="hidden text-base font-semibold tracking-tight font-display sm:inline">maestro</span>
        </div>
        <nav className="hidden items-center gap-1 md:flex">
          {navItems.map(({ path, label, icon: Icon, end }) => (
            <NavLink
              key={path}
              to={path}
              end={end}
              className={({ isActive }) =>
                cn(
                  'flex items-center gap-2 rounded-md px-3 py-1.5 text-sm font-medium transition-colors',
                  isActive
                    ? 'bg-hover text-primary'
                    : 'text-muted-foreground hover:bg-hover/50 hover:text-foreground',
                )
              }
            >
              <Icon className="h-4 w-4" />
              {label}
            </NavLink>
          ))}
        </nav>
      </div>
      <div className="flex items-center gap-2">
        <Button
          variant="ghost"
          size="sm"
          className="gap-1.5 text-muted-foreground"
          onClick={() => setCommandPaletteOpen(true)}
        >
          <Search className="h-3.5 w-3.5" />
          <span className="hidden sm:inline">Search</span>
          <kbd className="hidden rounded border border-border px-1.5 py-0.5 font-mono text-[10px] sm:inline">
            ⌘K
          </kbd>
        </Button>
        <Button size="sm" onClick={() => setRunDialogOpen(true)}>
          <Play className="h-3.5 w-3.5" />
          Run
        </Button>
      </div>
    </header>
  )
}
