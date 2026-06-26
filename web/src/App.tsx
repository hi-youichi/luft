import { Routes, Route, Outlet } from 'react-router-dom'
import { TopNav } from '@/components/layout/top-nav'
import { RunDialog } from '@/components/run-dialog'
import { TooltipProvider } from '@/components/ui/tooltip'
import { Dashboard } from '@/pages/dashboard'
import { RunsPage } from '@/pages/runs'
import { RunDetailPage } from '@/pages/run-detail'
import { WorkflowsPage } from '@/pages/workflows'
import { BackendsPage } from '@/pages/backends'

function RootLayout() {
  return (
    <div className="min-h-screen bg-bg-base">
      <TopNav />
      <main className="mx-auto max-w-7xl px-6 py-6">
        <Outlet />
      </main>
      <RunDialog />
    </div>
  )
}

export default function App() {
  return (
    <TooltipProvider>
      <Routes>
        <Route element={<RootLayout />}>
          <Route index element={<Dashboard />} />
          <Route path="runs" element={<RunsPage />} />
          <Route path="runs/:runId" element={<RunDetailPage />} />
          <Route path="workflows" element={<WorkflowsPage />} />
          <Route path="backends" element={<BackendsPage />} />
        </Route>
      </Routes>
    </TooltipProvider>
  )
}
