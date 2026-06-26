import { cn } from '@/lib/utils'
import { Card } from '@/components/ui/card'

interface StatCardProps {
  value: string | number
  label: string
  className?: string
}

export function StatCard({ value, label, className }: StatCardProps) {
  return (
    <Card className={cn('p-4', className)}>
      <div className="text-2xl font-bold font-display text-foreground">{value}</div>
      <div className="mt-1 text-xs text-muted-foreground">{label}</div>
    </Card>
  )
}
