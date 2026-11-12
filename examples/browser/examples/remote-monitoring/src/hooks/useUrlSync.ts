import { useEffect } from 'react'

/// The page's query with `params` set, or removed when null. The other parameters carry over, such
/// as the access mode a link asked for.
export function pageHref(params: Record<string, string | null>): string {
  const sp = new URLSearchParams(window.location.search)
  for (const [key, value] of Object.entries(params)) {
    if (value !== null) sp.set(key, value)
    else sp.delete(key)
  }
  return `?${sp.toString()}`
}

export function useUrlSync(params: Record<string, string | null>) {
  useEffect(() => {
    window.history.replaceState(null, '', `${pageHref(params)}${window.location.hash}`)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, Object.values(params))
}
