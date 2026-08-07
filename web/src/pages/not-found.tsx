import { Link } from 'react-router-dom'
import { FileQuestion } from 'lucide-react'
import { Button } from '@/components/ui/button'

export function NotFoundPage() {
  return (
    <div className="flex min-h-[400px] flex-col items-center justify-center gap-6 text-center">
      <div className="flex h-16 w-16 items-center justify-center rounded-2xl bg-muted/50">
        <FileQuestion className="h-7 w-7 text-muted-foreground" />
      </div>
      <div className="space-y-1">
        <h1 className="text-2xl font-bold font-display">404</h1>
        <p className="text-sm text-muted-foreground">Page not found</p>
      </div>
      <Button variant="outline" size="sm" asChild>
        <Link to="/">Back to Dashboard</Link>
      </Button>
    </div>
  )
}
