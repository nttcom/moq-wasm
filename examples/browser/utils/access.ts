/// How the examples reach the relay: as the public anonymous app (namespace root `anon`, open to
/// everyone), or as a temporary app of their own from the token mint (services/token-mint), whose
/// root only the people a page's link is shared with know. The choice is made on the example list and
/// kept in localStorage, so every example follows it.

import type { MoqConnectOptions } from 'moq-wasm'

import { offeredVersions } from './moqtVersion'
import { LOCAL_HOSTS, resolveServiceUrl } from './serviceUrl'

export type AccessMode = 'temporary' | 'public'

export type TemporaryToken = {
  token: string
  appId: string
  /// Unix seconds.
  expiresAt: number
}

export type Access =
  | { kind: 'public'; namespaceRoot: string }
  | { kind: 'temporary'; namespaceRoot: string; current: () => TemporaryToken }
  /// A token given in `?jwt=`, such as one the console signed for a registered app. It is used as is.
  | { kind: 'given'; namespaceRoot: string; token: string }
  /// Temporary mode without a usable token; `namespaceRoot` is the public root's, so nothing works
  /// until the page is reloaded or the mode is changed.
  | { kind: 'unavailable'; namespaceRoot: string; reason: string }

const PUBLIC_NAMESPACE_ROOT = 'anon'

const MODE_KEY = 'moqt.accessMode'
/// The newest token of the app the examples last joined, shared by every tab.
const TOKEN_KEY = 'moqt.temporaryToken'
/// The app this tab joined, kept across its reloads; a fragment naming another app is a shared link.
const TAB_APP_KEY = 'moqt.tabAppId'
const TOKEN_FRAGMENT = 'jwt'
/// A token is refreshed this long before it expires, so a reconnect never presents an expired one.
const REFRESH_MARGIN_SECONDS = 5 * 60

const TOKEN_MINT_URLS = {
  param: 'tokenMintUrl',
  local: 'http://localhost:8083',
  cloud: 'https://token-mint.moqt.research.skyway.io'
}

/// The stored choice. Without one, a page served from this machine talks to local relays nobody else
/// reaches, so it starts public; a deployed page starts temporary.
function readAccessMode(): AccessMode {
  const stored = localStorage.getItem(MODE_KEY)
  if (stored === 'temporary' || stored === 'public') {
    return stored
  }
  return LOCAL_HOSTS.has(window.location.hostname) ? 'public' : 'temporary'
}

export function writeAccessMode(mode: AccessMode): void {
  localStorage.setItem(MODE_KEY, mode)
}

/// A new temporary app, replacing the stored one.
export async function startNewTemporaryApp(): Promise<TemporaryToken> {
  const issued = await callMint('/tokens', undefined)
  localStorage.setItem(TOKEN_KEY, JSON.stringify(issued))
  return issued
}

/// The page's link with the current temporary token in its fragment: opening it joins the same app.
export function shareUrl(token: TemporaryToken): string {
  const url = new URL(window.location.href)
  url.hash = `${TOKEN_FRAGMENT}=${token.token}`
  return url.toString()
}

/// The page's access, resolved once when the module loads. In temporary mode it adopts a token from
/// the link's fragment, otherwise uses the stored one or issues a new app, and keeps refreshing the
/// token while the page is open.
export const access: Access = await resolveAccess()

export const accessMode: AccessMode = access.kind === 'public' ? 'public' : 'temporary'
document.documentElement.dataset.accessMode = accessMode

/// For `MoqSession.connect`'s `token`: the current token on every connection, including the
/// reconnect that follows the relay closing a session whose token expired.
export function connectToken(): string | (() => Promise<string>) | undefined {
  return access.kind === 'temporary' ? async () => access.current().token : bearerToken()
}

export function connectOptions(): MoqConnectOptions {
  return { token: connectToken(), versions: offeredVersions() }
}

/// For an HTTP service that authorizes with the relay's token, such as the recorder.
export function bearerToken(): string | undefined {
  switch (access.kind) {
    case 'given':
      return access.token
    case 'temporary':
      return access.current().token
    default:
      return undefined
  }
}

/// For a bot service (utils/botRoom.ts): the current temporary token, which it can keep refreshing on
/// the token mint. A token given in the URL is not one the token mint refreshes, so it is not handed
/// over.
export function temporaryAppToken(): string | undefined {
  return access.kind === 'temporary' ? access.current().token : undefined
}

export function withNamespaceRoot(namespace: string[]): string[] {
  return namespace[0] === PUBLIC_NAMESPACE_ROOT ? [access.namespaceRoot, ...namespace.slice(1)] : namespace
}

/// Rewrites the namespace inputs a page's markup fills with a public default, such as `anon/vc/aa`.
export function applyNamespaceRootToInputs(): void {
  if (access.namespaceRoot === PUBLIC_NAMESPACE_ROOT) {
    return
  }
  for (const input of document.querySelectorAll<HTMLInputElement>('input[type="text"]')) {
    const parts = input.value.split('/')
    if (parts.length > 1 && parts[0] === PUBLIC_NAMESPACE_ROOT) {
      input.value = withNamespaceRoot(parts).join('/')
    }
  }
}

async function resolveAccess(): Promise<Access> {
  const given = new URLSearchParams(window.location.search).get(TOKEN_FRAGMENT)?.trim()
  if (given) {
    return { kind: 'given', token: given, namespaceRoot: appIdOf(given) ?? PUBLIC_NAMESPACE_ROOT }
  }
  const fragment = fragmentToken()
  if (fragment && fragment.appId !== sessionStorage.getItem(TAB_APP_KEY)) {
    // A link someone shared: it joins its app, and so do the examples opened after it.
    if (fragment.expiresAt <= nowSeconds()) {
      return unavailable('この共有リンクは期限が切れています。共有した人に新しいリンクをもらってください')
    }
    writeAccessMode('temporary')
    localStorage.setItem(TOKEN_KEY, JSON.stringify(newer(fragment, storedToken())))
  } else if (!fragment && readAccessMode() === 'public') {
    return { kind: 'public', namespaceRoot: PUBLIC_NAMESPACE_ROOT }
  }
  // A reloaded tab stays in the app its fragment names, whatever the others chose since.
  let token = newer(fragment, storedToken())
  try {
    if (!token || token.expiresAt <= nowSeconds()) {
      if (fragment) {
        return unavailable('このリンクは期限が切れています。一覧ページでトークンを作り直してください')
      }
      token = await startNewTemporaryApp()
    } else if (token.expiresAt - nowSeconds() < REFRESH_MARGIN_SECONDS) {
      const stillValid = token
      token = await refresh(stillValid).catch((error) => {
        console.warn('[access] temporary token refresh failed', error)
        return stillValid
      })
    }
  } catch (error) {
    console.warn('[access] temporary token unavailable', error)
    return unavailable('一時トークンを取得できませんでした。再読み込みするか、一覧ページで Public に切り替えてください')
  }
  return joinTemporaryApp(token)
}

/// Keeps this tab's token in memory, taking a later one of the same app from another tab, and
/// refreshes it shortly before it expires.
function joinTemporaryApp(joined: TemporaryToken): Access {
  let current = joined
  sessionStorage.setItem(TAB_APP_KEY, current.appId)
  writeTokenFragment(current)
  const latest = () => {
    current = newer(current, storedToken()) ?? current
    return current
  }
  const schedule = () => {
    const delaySeconds = Math.max(latest().expiresAt - REFRESH_MARGIN_SECONDS - nowSeconds(), 10)
    window.setTimeout(async () => {
      const token = latest()
      if (token.expiresAt <= nowSeconds()) {
        // An expired token cannot be refreshed any more.
        return
      }
      if (token.expiresAt - nowSeconds() < REFRESH_MARGIN_SECONDS) {
        await refresh(token)
          .then((refreshed) => (current = refreshed))
          .catch((error) => console.warn('[access] temporary token refresh failed', error))
      }
      schedule()
    }, delaySeconds * 1000)
  }
  schedule()
  return { kind: 'temporary', namespaceRoot: joined.appId, current: latest }
}

function unavailable(reason: string): Access {
  return { kind: 'unavailable', namespaceRoot: PUBLIC_NAMESPACE_ROOT, reason }
}

/// The later of two tokens of the same app; with different apps, `own`.
function newer(own: TemporaryToken | undefined, other: TemporaryToken | undefined): TemporaryToken | undefined {
  if (!own || !other || own.appId !== other.appId) {
    return own ?? other
  }
  return other.expiresAt > own.expiresAt ? other : own
}

async function refresh(token: TemporaryToken): Promise<TemporaryToken> {
  const refreshed = await callMint('/tokens/refresh', { token: token.token })
  // Another tab may have joined another app since; only the same app's entry is replaced.
  if (storedToken()?.appId === refreshed.appId) {
    localStorage.setItem(TOKEN_KEY, JSON.stringify(refreshed))
  }
  writeTokenFragment(refreshed)
  return refreshed
}

async function callMint(path: string, body: unknown): Promise<TemporaryToken> {
  const mintUrl = resolveServiceUrl(window.location.search, window.location.href, TOKEN_MINT_URLS)
  const response = await fetch(`${mintUrl}${path}`, {
    method: 'POST',
    headers: body === undefined ? {} : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body)
  })
  const answer = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(answer.error ?? `token mint answered ${response.status}`)
  }
  return answer as TemporaryToken
}

/// Keeps the current token in the address bar, so copying it shares the app.
function writeTokenFragment(token: TemporaryToken): void {
  window.history.replaceState(window.history.state, '', shareUrl(token))
}

function fragmentToken(): TemporaryToken | undefined {
  const token = new URLSearchParams(window.location.hash.slice(1)).get(TOKEN_FRAGMENT)
  const appId = token ? appIdOf(token) : undefined
  const expiresAt = token ? expiryOf(token) : undefined
  return token && appId && expiresAt !== undefined ? { token, appId, expiresAt } : undefined
}

function storedToken(): TemporaryToken | undefined {
  try {
    const parsed = JSON.parse(localStorage.getItem(TOKEN_KEY) ?? 'null')
    return parsed && typeof parsed.token === 'string' ? (parsed as TemporaryToken) : undefined
  } catch {
    return undefined
  }
}

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000)
}

function payloadOf(token: string): Record<string, unknown> | undefined {
  const payload = token.split('.')[1]
  if (!payload) {
    return undefined
  }
  try {
    return JSON.parse(atob(payload.replace(/-/g, '+').replace(/_/g, '/')))
  } catch {
    return undefined
  }
}

/// Decoded only to name the namespace root the page requests; the relay verifies the token itself.
function appIdOf(token: string): string | undefined {
  const appId = payloadOf(token)?.appId
  return typeof appId === 'string' && appId !== '' ? appId : undefined
}

function expiryOf(token: string): number | undefined {
  const exp = payloadOf(token)?.exp
  return typeof exp === 'number' ? exp : undefined
}
