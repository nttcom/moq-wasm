const ANON_ROOT = 'anon'

// The relay accepts a tokenless session as the anonymous app (namespace root
// `anon`). Passing `?jwt=<token>` connects as that token's appId instead; the
// namespace root then follows the appId decoded from the token. Decoding here
// is only used to build the namespace the client requests — the relay verifies
// the token itself and authorizes against the appId it verified.
export type CallAuth = {
  token?: string
  namespaceRoot: string
}

export function readCallAuth(): CallAuth {
  if (typeof window === 'undefined') {
    return { namespaceRoot: ANON_ROOT }
  }
  const token = new URLSearchParams(window.location.search).get('jwt')?.trim()
  if (!token) {
    return { namespaceRoot: ANON_ROOT }
  }
  return { token, namespaceRoot: appIdFromToken(token) ?? ANON_ROOT }
}

function appIdFromToken(token: string): string | null {
  const payload = token.split('.')[1]
  if (!payload) {
    return null
  }
  try {
    const json = atob(payload.replace(/-/g, '+').replace(/_/g, '/'))
    const appId = (JSON.parse(json) as { appId?: unknown }).appId
    return typeof appId === 'string' && appId !== '' ? appId : null
  } catch {
    return null
  }
}
