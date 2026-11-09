/// An HTTP service a page uses next to its relay.
export type ServiceUrls = {
  /// The query parameter that overrides both.
  param: string
  local: string
  cloud: string
}

export const LOCAL_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]'])

const DEPLOYED_HOST_SUFFIX = '.moqt.research.skyway.io'

/// The first of the `names` query parameters set in `search`, if it is a URL on this machine
/// (`LOCAL_HOSTS`) or a deployed host. A page sends its token to the URLs it uses, so a link must
/// not point it at any other host.
export function urlFromQuery(search: string, names: string[]): string | undefined {
  const params = new URLSearchParams(search)
  for (const name of names) {
    const value = params.get(name)?.trim()
    if (!value) {
      continue
    }
    const hostname = URL.canParse(value) ? new URL(value).hostname : ''
    if (LOCAL_HOSTS.has(hostname) || hostname.endsWith(DEPLOYED_HOST_SUFFIX)) {
      return value
    }
    console.warn(`[serviceUrl] ignored ?${name}=${value}: only this machine and *${DEPLOYED_HOST_SUFFIX} are allowed`)
    return undefined
  }
  return undefined
}

/// The `param` query parameter (see `urlFromQuery`), or the service that reaches the relay:
/// the local one for a local relay, the deployed one otherwise.
export function resolveServiceUrl(search: string, relayUrl: string, urls: ServiceUrls): string {
  const configured = urlFromQuery(search, [urls.param])
  if (configured) {
    return configured.replace(/\/+$/, '')
  }
  try {
    return LOCAL_HOSTS.has(new URL(relayUrl).hostname) ? urls.local : urls.cloud
  } catch {
    return urls.cloud
  }
}
