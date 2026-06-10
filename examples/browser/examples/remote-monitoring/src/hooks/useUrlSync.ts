import { useEffect } from 'react'

export function useUrlSync(params: Record<string, string | null>) {
  useEffect(() => {
    const sp = new URLSearchParams(window.location.search)
    for (const [key, value] of Object.entries(params)) {
      if (value !== null) sp.set(key, value)
      else sp.delete(key)
    }
    window.history.replaceState(null, '', `?${sp.toString()}`)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, Object.values(params))
}
