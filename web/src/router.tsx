import { lazy, Suspense } from 'react'
import type { ReactNode } from 'react'
import { createBrowserRouter, Outlet } from 'react-router-dom'
import { TopNav } from '@/components/layout/top-nav'
import { RunDialog } from '@/components/run-dialog'
import { CommandPalette } from '@/components/layout/command-palette'
import { TooltipProvider } from '@/components/ui/tooltip'
import { RouteErrorBoundary } from '@/components/layout/route-error-boundary'
import { RouteLoading } from '@/components/layout/route-loading'
import { ScrollToTop } from '@/components/layout/scroll-to-top'
import { useGlobalShortcuts } from '@/hooks/useKeyboardShortcuts'
import { NotFoundPage } from '@/pages/not-found'

const Dashboard = lazy(() =>
  import('@/pages/dashboard').then((m) => ({ default: m.Dashboard })),
)
const RunsPage = lazy(() =>
  import('@/pages/runs').then((m) => ({ default: m.RunsPage })),
)
const RunDetailPage = lazy(() =>
  import('@/pages/run-detail').then((m) => ({ default: m.RunDetailPage })),
)
const WorkflowsPage = lazy(() =>
  import('@/pages/workflows').then((m) => ({ default: m.WorkflowsPage })),
)
const BackendsPage = lazy(() =>
  import('@/pages/backends').then((m) => ({ default: m.BackendsPage })),
)
const MetricsPage = lazy(() =>
  import('@/pages/metrics').then((m) => ({ default: m.MetricsPage })),
)
const LivePage = lazy(() =>
  import('@/pages/live').then((m) => ({ default: m.LivePage })),
)
const ReportsPage = lazy(() =>
  import('@/pages/reports').then((m) => ({ default: m.ReportsPage })),
)

function RootLayout() {
  useGlobalShortcuts()

  return (
    <div className="min-h-screen bg-bg-base">
      <ScrollToTop />
      <TopNav />
      <main className="mx-auto max-w-7xl px-6 py-6">
        <Outlet />
      </main>
      <RunDialog />
      <CommandPalette />
    </div>
  )
}

function withSuspense(element: ReactNode) {
  return <Suspense fallback={<RouteLoading />}>{element}</Suspense>
}

export const router = createBrowserRouter([
  {
    element: (
      <TooltipProvider>
        <RootLayout />
      </TooltipProvider>
    ),
    errorElement: <RouteErrorBoundary />,
    children: [
      { index: true, element: withSuspense(<Dashboard />) },
      { path: 'runs', element: withSuspense(<RunsPage />) },
      { path: 'runs/:runId', element: withSuspense(<RunDetailPage />) },
      { path: 'workflows', element: withSuspense(<WorkflowsPage />) },
      { path: 'backends', element: withSuspense(<BackendsPage />) },
      { path: 'metrics', element: withSuspense(<MetricsPage />) },
      { path: 'live', element: withSuspense(<LivePage />) },
      { path: 'reports', element: withSuspense(<ReportsPage />) },
      { path: '*', element: <NotFoundPage /> },
    ],
  },
])