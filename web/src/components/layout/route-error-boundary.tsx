import { isRouteErrorResponse, useNavigate, useRouteError } from 'react-router-dom'
import { AlertTriangle, RotateCcw, Home } from 'lucide-react'
import { Button } from '@/components/ui/button'

export function RouteErrorBoundary() {
  const error = useRouteError()
  const navigate = useNavigate()

  let message = '渲染过程中发生了意外错误'
  if (isRouteErrorResponse(error)) {
    message =
      error.status === 404
        ? '页面不存在'
        : `${error.status} ${error.statusText || error.data}`
  } else if (error instanceof Error) {
    message = error.message
  }

  return (
    <div className="flex min-h-[60vh] flex-col items-center justify-center gap-4 text-center">
      <div className="flex h-14 w-14 items-center justify-center rounded-xl bg-destructive/10">
        <AlertTriangle className="h-7 w-7 text-destructive" />
      </div>
      <div className="space-y-1">
        <h2 className="text-lg font-semibold font-display">页面加载出错</h2>
        <p className="max-w-md text-sm text-muted-foreground">{message}</p>
      </div>
      <div className="flex gap-2">
        <Button size="sm" variant="outline" onClick={() => navigate(0)}>
          <RotateCcw className="h-3.5 w-3.5" />
          重试
        </Button>
        <Button size="sm" variant="ghost" onClick={() => navigate('/')}>
          <Home className="h-3.5 w-3.5" />
          返回首页
        </Button>
      </div>
    </div>
  )
}
