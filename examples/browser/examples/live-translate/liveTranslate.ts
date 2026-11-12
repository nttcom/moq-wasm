import { access, bearerToken, renewedTemporaryToken, withNamespaceRoot } from '../../utils/access'
import { type ServiceUrls, resolveServiceUrl } from '../../utils/serviceUrl'

export const BROADCAST_ID_PARAM = 'id'
const TRANSLATED_NAMESPACE_SUFFIX = 'translated'
const TOKEN_FRAGMENT = 'jwt'

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

/// Has the translator translate the broadcasts of the page's temporary app. The translator reaches
/// the app only with one of its tokens and keeps the one it is handed for that token's lifetime, so
/// it gets a newly signed one, good for an hour. The event stream held until `release` keeps the
/// translator's instance running. A public broadcast needs neither: the translator watches
/// `anon/live-translate` by itself.
export async function requestTranslation(relayUrl: string): Promise<TranslationRequest> {
  if (access.kind !== 'temporary') {
    return { notice: undefined, release: () => {} }
  }
  const translatorUrl = resolveServiceUrl(window.location.search, relayUrl, TRANSLATOR_URLS)
  const response = await fetch(`${translatorUrl}/sessions`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${await renewedTemporaryToken()}` }
  })
  const answer = await response.json().catch(() => ({}))
  if (!response.ok) {
    return {
      notice: REFUSALS[answer.error] ?? `翻訳を始められませんでした（${answer.error ?? response.status}）`,
      release: () => {}
    }
  }
  const events = new EventSource(`${translatorUrl}/events`)
  return {
    notice: answer.openHours && !answer.open ? `翻訳は稼働時間外です（稼働時間: ${answer.openHours}）` : undefined,
    release: () => events.close()
  }
}

/// The Viewer's link for `broadcastId`, carrying the page's token so that whoever opens it joins the
/// same app.
export function viewerUrl(broadcastId: string, relayUrl: string, from: string = location.href): string {
  const url = new URL('../viewer/index.html', from)
  url.search = ''
  url.searchParams.set(BROADCAST_ID_PARAM, broadcastId)
  url.searchParams.set('relayUrl', relayUrl)
  const token = bearerToken()
  url.hash = token ? `${TOKEN_FRAGMENT}=${token}` : ''
  return url.toString()
}
