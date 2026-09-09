const ANON_ISSUER_URL_PARAM_NAMES = ['anonIssuerUrl', 'issuerUrl'] as const

export function resolveAnonIssuerUrl(): string | null {
  if (typeof window === 'undefined') {
    return null
  }
  const params = new URLSearchParams(window.location.search)
  for (const name of ANON_ISSUER_URL_PARAM_NAMES) {
    const value = params.get(name)?.trim()
    if (value) {
      return value
    }
  }
  return null
}

export async function fetchAnonToken(issuerUrl: string): Promise<string> {
  const response = await fetch(issuerUrl, { method: 'POST' })
  if (!response.ok) {
    throw new Error(`anon issuer ${issuerUrl} responded with ${response.status}`)
  }
  const body = (await response.json()) as { token?: unknown }
  if (typeof body.token !== 'string' || body.token === '') {
    throw new Error(`anon issuer ${issuerUrl} returned no token`)
  }
  return body.token
}

/**
 * Returns the token to present in CLIENT_SETUP. When no issuer is configured
 * the relay is expected to run with AUTH_DISABLED and no token is sent; when
 * one is configured, failing to obtain a token is an error rather than a
 * silent fallback to an unauthenticated session.
 */
export async function resolveAuthToken(): Promise<string | undefined> {
  const issuerUrl = resolveAnonIssuerUrl()
  if (!issuerUrl) {
    return undefined
  }
  const token = await fetchAnonToken(issuerUrl)
  console.info('[moqt][auth] obtained anon token', { issuerUrl })
  return token
}
