/// Carries the page's query (a relay URL, a token mint URL) over to `href`.
export function withPageQuery(href: string): string {
  const url = new URL(href, window.location.href)
  for (const [name, value] of new URLSearchParams(window.location.search)) {
    url.searchParams.set(name, value)
  }
  return url.toString()
}
