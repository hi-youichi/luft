export function formatTokens(tokens: number): string {
  if (tokens >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`
  if (tokens >= 1000) return `${(tokens / 1000).toFixed(1)}k`
  return String(tokens)
}

export function formatElapsed(ms: number): string {
  if (ms < 60_000) {
    const s = Math.floor(ms / 1000)
    return `${s}s`
  }
  const m = Math.floor(ms / 60_000)
  if (m < 60) {
    const s = Math.floor((ms % 60_000) / 1000)
    return `${m}:${String(s).padStart(2, '0')}`
  }
  const h = Math.floor(m / 60)
  return `${h}h ${m % 60}m`
}

export function formatRelativeTime(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime()
  const min = Math.floor(diff / 60_000)
  if (min < 1) return '刚刚'
  if (min < 60) return `${min}分钟前`
  const h = Math.floor(min / 60)
  if (h < 24) return `${h}小时前`
  const d = Math.floor(h / 24)
  return `${d}天前`
}

export function formatTime(iso: string): string {
  const d = new Date(iso)
  return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })
}
