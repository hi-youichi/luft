import { useCallback, useEffect } from 'react'
import { useUIStore } from '@/stores/ui'

export interface KeyboardShortcutHandlers {
  onSearch?: () => void
  onPrev?: () => void
  onNext?: () => void
  onClose?: () => void
  onToggleDensity?: () => void
  onTogglePause?: () => void
  onExport?: () => void
}

const shortcutMap: Record<string, keyof KeyboardShortcutHandlers> = {
  '/': 'onSearch',
  'j': 'onNext',
  'k': 'onPrev',
  'Escape': 'onClose',
  'd': 'onToggleDensity',
  'p': 'onTogglePause',
  'e': 'onExport',
}

export function useKeyboardShortcuts(handlers: KeyboardShortcutHandlers, enabled = true) {
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      if (!enabled) return

      const target = e.target as HTMLElement
      const isTyping =
        target.tagName === 'INPUT' ||
        target.tagName === 'TEXTAREA' ||
        target.isContentEditable

      if (isTyping && e.key !== 'Escape') return

      const handlerKey = shortcutMap[e.key]
      if (!handlerKey) return

      const handler = handlers[handlerKey]
      if (handler) {
        e.preventDefault()
        handler()
      }
    },
    [handlers, enabled],
  )

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown)
    return () => window.removeEventListener('keydown', handleKeyDown)
  }, [handleKeyDown])
}

export function useGlobalShortcuts() {
  const setCommandPaletteOpen = useUIStore((s) => s.setCommandPaletteOpen)
  const commandPaletteOpen = useUIStore((s) => s.commandPaletteOpen)
  const setRunDialogOpen = useUIStore((s) => s.setRunDialogOpen)

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault()
        setCommandPaletteOpen(!commandPaletteOpen)
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [setCommandPaletteOpen, commandPaletteOpen, setRunDialogOpen])
}
