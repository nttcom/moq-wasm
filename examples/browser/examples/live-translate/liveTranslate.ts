export const BROADCAST_ID_PARAM = 'id'
const PUBLIC_NAMESPACE_PREFIX = ['anon', 'live-translate']
const TRANSLATED_NAMESPACE_SUFFIX = 'translated'

export type BroadcastRole = 'streamer' | 'viewer'

/// Served by the live-translate service, a page gets a token for its one
/// broadcast and holds the service's event stream so the instance stays up
/// while the page is in use. Served without one (vite, GitHub Pages), the
/// broadcast is public under `anon/live-translate` and needs no token.
export type BroadcastAccess = {
  namespace: string[]
  relayUrl: string | undefined
  token: (() => Promise<string>) | undefined
  /// Set while the translator is outside its open hours, for the page to
  /// show before a viewer picks a language.
  closedNotice: string | undefined
  release(): void
}

export async function openBroadcastAccess(broadcastId: string, role: BroadcastRole): Promise<BroadcastAccess> {
  const grant = await fetchGrant(broadcastId, role)
  if (!grant) {
    return {
      namespace: [...PUBLIC_NAMESPACE_PREFIX, broadcastId],
      relayUrl: undefined,
      token: undefined,
      closedNotice: undefined,
      release: () => {}
    }
  }
  const events = new EventSource('/events')
  return {
    namespace: grant.namespace,
    relayUrl: grant.relayUrl,
    token: async () => (await fetchGrant(broadcastId, role))?.token ?? grant.token,
    closedNotice: grant.openHours && !grant.open ? `翻訳は稼働時間外です（稼働時間: ${grant.openHours}）` : undefined,
    release: () => events.close()
  }
}

type Grant = { token: string; namespace: string[]; relayUrl: string; openHours: string | null; open: boolean }

/// Only an answer that shows the page is served without the service, a 404
/// or a dev server's page itself, means the broadcast is public. Any other
/// failure, such as an expired IAP session (a redirect to the login page), is
/// reported rather than quietly publishing in public.
async function fetchGrant(broadcastId: string, role: BroadcastRole): Promise<Grant | undefined> {
  const response = await fetch(`/token?id=${encodeURIComponent(broadcastId)}&role=${role}`, { redirect: 'manual' })
  const json = response.headers.get('content-type')?.includes('application/json') ?? false
  if (response.ok && json) {
    const body = (await response.json()) as Omit<Grant, 'namespace'> & { namespace: string }
    return { ...body, namespace: body.namespace.split('/') }
  }
  if (response.status === 404 || (response.ok && !json)) {
    return undefined
  }
  if (response.type === 'opaqueredirect') {
    throw new Error('ログインの期限が切れています。ページを読み込み直してください')
  }
  const error = json ? ((await response.json()) as { error?: string }).error : undefined
  throw new Error(`relay token: ${error ?? response.statusText ?? response.status}`)
}

export function translatedNamespace(broadcastNamespace: string[]): string[] {
  return [...broadcastNamespace, TRANSLATED_NAMESPACE_SUFFIX]
}

export function viewerUrl(broadcastId: string, relayUrl: string, from: string = location.href): string {
  const url = new URL('../viewer/index.html', from)
  url.search = ''
  url.searchParams.set(BROADCAST_ID_PARAM, broadcastId)
  url.searchParams.set('relayUrl', relayUrl)
  return url.toString()
}
