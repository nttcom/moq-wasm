import { TOKEN_FRAGMENT, access, bearerToken, renewedTemporaryToken, withNamespaceRoot } from '../../utils/access'
import { LOCAL_HOSTS, type ServiceUrls, resolveServiceUrl } from '../../utils/serviceUrl'

export const BROADCAST_ID_PARAM = 'id'
const TRANSLATED_NAMESPACE_SUFFIX = 'translated'

const TRANSLATOR_URLS: ServiceUrls = {
  param: 'liveTranslateUrl',
  local: 'http://localhost:18090',
  cloud: 'https://moqt-relay-live-translate-2wygx2l4sq-an.a.run.app'
}

const REFUSALS: Record<string, string> = {
  full: '翻訳は満席です。しばらくしてから配信し直してください',
  relay_refused: '翻訳サービスが relay に接続できませんでした'
}

export function broadcastNamespace(broadcastId: string): string[] {
  return withNamespaceRoot(['anon', 'live-translate', broadcastId])
}

export function translatedNamespace(broadcastNamespace: string[]): string[] {
  return [...broadcastNamespace, TRANSLATED_NAMESPACE_SUFFIX]
}

export type TranslationRequest = {
  /// Why the broadcast is not translated now, for the streamer to show.
  notice: string | undefined
  release(): void
}

/// The translator reaches a temporary app only with one of its tokens and keeps the one it is
/// handed until it expires, so it gets a newly signed one. The event stream held until `release`
/// keeps the translator's instance running. A public broadcast needs neither: a translator
/// on this machine watches `anon/live-translate` by itself, and a deployed one does not translate
/// it. A failure is a notice, since the broadcast runs without its translation.
export async function requestTranslation(relayUrl: string): Promise<TranslationRequest> {
  if (access.kind !== 'temporary') {
    // Only a translator on this machine watches the public root.
    const local = URL.canParse(relayUrl) && LOCAL_HOSTS.has(new URL(relayUrl).hostname)
    return { notice: local ? undefined : '翻訳は一時トークンのモードでのみ使えます', release: () => {} }
  }
  const translatorUrl = resolveServiceUrl(window.location.search, relayUrl, TRANSLATOR_URLS)
  const refused = (reason: unknown): TranslationRequest => ({
    notice: REFUSALS[String(reason)] ?? `翻訳を始められませんでした（${reason}）`,
    release: () => {}
  })
  try {
    const response = await fetch(`${translatorUrl}/sessions`, {
      method: 'POST',
      headers: { Authorization: `Bearer ${await renewedTemporaryToken()}` }
    })
    const answer = await response.json().catch(() => ({}))
    if (!response.ok) {
      return refused(answer.error ?? response.status)
    }
    const events = new EventSource(`${translatorUrl}/events`)
    return {
      notice: answer.openHours && !answer.open ? `翻訳は稼働時間外です（稼働時間: ${answer.openHours}）` : undefined,
      release: () => events.close()
    }
  } catch (error) {
    return refused(error instanceof Error ? error.message : error)
  }
}

/// Carries the page's token, so whoever opens the link joins the same app.
export function viewerUrl(broadcastId: string, relayUrl: string, from: string = location.href): string {
  const url = new URL('../viewer/index.html', from)
  url.search = ''
  url.searchParams.set(BROADCAST_ID_PARAM, broadcastId)
  url.searchParams.set('relayUrl', relayUrl)
  const token = bearerToken()
  url.hash = token ? `${TOKEN_FRAGMENT}=${token}` : ''
  return url.toString()
}
