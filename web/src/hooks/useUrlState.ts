import { useCallback } from 'react'
import { useSearchParams } from 'react-router-dom'

export function useUrlState<T extends Record<string, string>>(
  defaults: T,
): {
  values: T
  setValue: (key: keyof T, value: string) => void
  setValues: (updates: Partial<T>) => void
  clear: () => void
} {
  const [searchParams, setSearchParams] = useSearchParams()

  const values = {} as T
  for (const key of Object.keys(defaults) as (keyof T)[]) {
    values[key] = (searchParams.get(key as string) ?? defaults[key]) as T[keyof T]
  }

  const setValue = useCallback(
    (key: keyof T, value: string) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev)
          if (value === defaults[key]) {
            next.delete(key as string)
          } else {
            next.set(key as string, value)
          }
          return next
        },
        { replace: true },
      )
    },
    [defaults, setSearchParams],
  )

  const setValues = useCallback(
    (updates: Partial<T>) => {
      setSearchParams(
        (prev) => {
          const next = new URLSearchParams(prev)
          for (const [key, value] of Object.entries(updates)) {
            if (value == null || value === defaults[key as keyof T]) {
              next.delete(key)
            } else {
              next.set(key, value)
            }
          }
          return next
        },
        { replace: true },
      )
    },
    [defaults, setSearchParams],
  )

  const clear = useCallback(() => {
    setSearchParams({}, { replace: true })
  }, [setSearchParams])

  return { values, setValue, setValues, clear }
}
