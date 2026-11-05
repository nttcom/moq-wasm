export const BROADCAST_ID_PARAM = 'id'
const BROADCAST_NAMESPACE_PREFIX = ['anon', 'live-translate']
const TRANSLATED_NAMESPACE_SUFFIX = 'translated'

export function broadcastNamespace(broadcastId: string): string[] {
  return [...BROADCAST_NAMESPACE_PREFIX, broadcastId]
}

export function translatedNamespace(broadcastId: string): string[] {
  return [...broadcastNamespace(broadcastId), TRANSLATED_NAMESPACE_SUFFIX]
}

export function viewerUrl(broadcastId: string, relayUrl: string, from: string = location.href): string {
  const url = new URL('../viewer/index.html', from)
  url.search = ''
  url.searchParams.set(BROADCAST_ID_PARAM, broadcastId)
  url.searchParams.set('relayUrl', relayUrl)
  return url.toString()
}
