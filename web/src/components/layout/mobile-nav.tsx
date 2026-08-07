import { useState } from 'react'
import { NavLink } from 'react-router-dom'
import { Menu, Play } from 'lucide-react'
import { Sheet, SheetContent, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Button } from '@/components/ui/button'
import { useUIStore } from '@/stores/ui'
import { navItems } from '@/lib/routes'
import { cn } from '@/lib/utils'

export function MobileNav() {
  const [open, setOpen] = useState(false)
  const setRunDialogOpen = useUIStore((s) => s.setRunDialogOpen)

  return (
    <>
      <Button
        variant="ghost"
        size="sm"
        className="md:hidden"
        onClick={() => setOpen(true)}
      >
        <Menu className="h-5 w-5" />
      </Button>
      <Sheet open={open} onOpenChange={setOpen}>
        <SheetContent side="left" className="w-72 p-0">
          <SheetHeader className="border-b border-border">
            <SheetTitle className="flex items-center gap-2">
              <div className="flex h-7 w-7 items-center justify-center rounded-md bg-primary/15">
                <span className="text-base font-bold text-primary font-display">M</span>
              </div>
              maestro
            </SheetTitle>
          </SheetHeader>
          <nav className="flex flex-col gap-1 p-3">
            {navItems.map(({ path, label, icon: Icon, end }) => (
              <NavLink
                key={path}
                to={path}
                end={end}
                onClick={() => setOpen(false)}
                className={({ isActive }) =>
                  cn(
                    'flex items-center gap-3 rounded-md px-3 py-2 text-sm font-medium transition-colors',
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
          <div className="border-t border-border p-3">
            <Button
              size="sm"
              className="w-full"
              onClick={() => {
                setOpen(false)
                setRunDialogOpen(true)
              }}
            >
              <Play className="h-3.5 w-3.5" />
              Run
            </Button>
          </div>
        </SheetContent>
      </Sheet>
    </>
  )
}
