import { Fragment } from 'react'
import { Link, useLocation } from 'react-router-dom'
import { ChevronRight, Home } from 'lucide-react'
import { routes, buildBreadcrumbs, type RouteKey } from '@/lib/routes'
import { cn } from '@/lib/utils'

function matchPath(
  pattern: string,
  pathname: string,
): Record<string, string> | null {
  const pp = pattern.split('/').filter(Boolean)
  const ap = pathname.split('/').filter(Boolean)
  if (pp.length !== ap.length) return null
  const params: Record<string, string> = {}
  for (let i = 0; i < pp.length; i++) {
    if (pp[i].startsWith(':')) {
      params[pp[i].slice(1)] = decodeURIComponent(ap[i])
    } else if (pp[i] !== ap[i]) {
      return null
    }
  }
  return params
}

function matchRoute(
  pathname: string,
): { key: RouteKey; params: Record<string, string> } | null {
  const entries = Object.entries(routes) as [RouteKey, (typeof routes)[RouteKey]][]
  const sorted = entries.sort((a, b) => {
    const aDynamic = (a[1].path.match(/:/g) || []).length
    const bDynamic = (b[1].path.match(/:/g) || []).length
    return aDynamic - bDynamic
  })
  for (const [key, route] of sorted) {
    const params = matchPath(route.path, pathname)
    if (params) return { key, params }
  }
  return null
}

export function Breadcrumbs({ className }: { className?: string }) {
  const { pathname } = useLocation()
  const match = matchRoute(pathname)

  if (!match || match.key === 'dashboard') return null

  const crumbs = buildBreadcrumbs(match.key, match.params)

  return (
    <nav
      className={cn(
        'flex items-center gap-1.5 text-sm text-muted-foreground',
        className,
      )}
    >
      <Link to="/" className="transition-colors hover:text-foreground">
        <Home className="h-3.5 w-3.5" />
      </Link>
      {crumbs.map((crumb, i) => {
        const isLast = i === crumbs.length - 1
        return (
          <Fragment key={i}>
            <ChevronRight className="h-3 w-3 shrink-0 text-muted-foreground/50" />
            {isLast ? (
              <span className="font-medium text-foreground">{crumb.label}</span>
            ) : (
              <Link to={crumb.to} className="transition-colors hover:text-foreground">
                {crumb.label}
              </Link>
            )}
          </Fragment>
        )
      })}
    </nav>
  )
}
