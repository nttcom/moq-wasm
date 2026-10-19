/// An HTTP service a page uses next to its relay.
export type ServiceUrls = {
  /// The query parameter that overrides both.
  param: string
  local: string
  cloud: string
}

const LOCAL_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]'])

/// The `param` query parameter, or the service that reaches the relay:
/// the local one for a local relay, the deployed one otherwise.
export function resolveServiceUrl(search: string, relayUrl: string, urls: ServiceUrls): string {
  const configured = new URLSearchParams(search).get(urls.param)?.trim()
  if (configured) {
    return configured.replace(/\/+$/, '')
  }
  try {
    return LOCAL_HOSTS.has(new URL(relayUrl).hostname) ? urls.local : urls.cloud
  } catch {
    return urls.cloud
  }
}
