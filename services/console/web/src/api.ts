export type ConsoleApp = {
  appId: string
  name: string | null
  createdAt: number | null
  rotatedAt: number | null
}

export type Me = {
  email: string | null
  relayUrl: string | null
}

export type AppWithSecret = {
  app: ConsoleApp
  secret: string
}

export type TokenScope = {
  publish?: string
  subscribe?: string
  ttlSeconds: number
}

export type IssuedToken = {
  token: string
  expiresAt: number
}

/** A metric's value per bucket of `t`; `null` where it has none. */
export type Series = {
  t: number[]
  series: Record<string, (number | null)[]>
}

/**
 * The app's traffic in a range, counted on its client sessions. "Sent" is relay → client (the
 * subscribers' downlink), "received" client → relay (the publishers' uplink).
 */
export type Usage = {
  traffic: Series
  relays: { relay_id: string; sent_bytes: number; received_bytes: number; sessions: number }[]
  namespaces: { namespace: string; sent_bytes: number; received_bytes: number }[]
  sessions: {
    relay_id: string
    session_id: string
    sent_bytes: number
    received_bytes: number
    rtt_ms: number
    first_seen_ms: number
    last_seen_ms: number
  }[]
}

export type RecordingState = 'starting' | 'recording' | 'stopping' | 'finished' | 'failed'

/** An MP4 room recording of the app, as the recorder lists it. */
export type Recording = {
  id: string
  requested_at_ms: number | null
  /** `<appId>/<room>`. */
  namespace_prefix: string
  state: RecordingState
  error: string | null
  members: { namespace: string; started_at_ms: number; finished: boolean; error: string | null }[]
  /** `name` is the file's key below the recording: `<namespace>/[<track>/]<start ms>-<n>.mp4` or `composite.mp4`. */
  files: { name: string; size_bytes: number }[]
}

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string
  ) {
    super(code)
  }
}

const messages: Record<string, string> = {
  unauthenticated: '本人を確認できませんでした。ページを開き直してください。',
  forbidden: 'この操作は受け付けられませんでした。ページを開き直してください。',
  registry_unavailable: '登録簿を読み書きできませんでした。しばらくしてから試してください。',
  no_app: 'この app は見つかりません。ページを開き直してください。',
  too_many_apps: 'app は 1 人 20 個まで作れます。使っていない app を削除してください。',
  invalid_name: '名前は 1〜64 文字で、改行などの制御文字は使えません。',
  invalid_path: 'パスは「/」で区切った空でない名前で、255 文字までです。',
  no_scope: '配信か視聴の少なくとも一方を許可してください。',
  invalid_ttl: '期限は 24 時間までで指定してください。',
  invalid_request: 'リクエストの形が正しくありません。',
  invalid_range: '期間は 7 日以内で指定してください。',
  observability_unavailable: '通信量を読めませんでした。しばらくしてから試してください。',
  recorder_unavailable: '録画を読めませんでした。しばらくしてから試してください。'
}

export function describeError(error: unknown): string {
  if (error instanceof ApiError) {
    return messages[error.code] ?? `予期しないエラーです（${error.status} ${error.code}）。`
  }
  return 'コンソールに接続できませんでした。ページを開き直してください。'
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = { 'X-Console-Request': '1' }
  if (body !== undefined) {
    headers['Content-Type'] = 'application/json'
  }
  const response = await fetch(path, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body)
  })
  if (!response.ok) {
    const data = (await response.json().catch(() => ({}))) as { error?: string }
    throw new ApiError(response.status, data.error ?? 'unknown')
  }
  return (response.status === 204 ? undefined : await response.json()) as T
}

const appPath = (appId: string) => `/api/apps/${encodeURIComponent(appId)}`

export const api = {
  me: () => request<Me>('GET', '/api/me'),
  listApps: () => request<{ apps: ConsoleApp[] }>('GET', '/api/apps'),
  createApp: (name: string) => request<AppWithSecret>('POST', '/api/apps', { name }),
  issueToken: (appId: string, scope: TokenScope) => request<IssuedToken>('POST', `${appPath(appId)}/tokens`, scope),
  rotateSecret: (appId: string) => request<AppWithSecret>('POST', `${appPath(appId)}/secret`),
  deleteApp: (appId: string) => request<void>('DELETE', appPath(appId)),
  usage: (appId: string, fromMs: number, toMs: number, points: number) =>
    request<Usage>('GET', `${appPath(appId)}/usage?from=${fromMs}&to=${toMs}&points=${points}`),
  recordings: (appId: string) => request<{ recordings: Recording[] }>('GET', `${appPath(appId)}/recordings`)
}

/** The console path that redirects to the file's signed Cloud Storage URL. */
export function recordingFileUrl(appId: string, id: string, name: string): string {
  return `${appPath(appId)}/recordings/${encodeURIComponent(id)}/file?name=${encodeURIComponent(name)}`
}

export function formatBytes(bytes: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let unit = 0
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000
    unit += 1
  }
  const digits = value >= 100 || unit === 0 ? 0 : 1
  return `${Number(value.toFixed(digits))} ${units[unit]}`
}

export function formatUnixTime(seconds: number | null): string {
  if (seconds === null) {
    return '—'
  }
  const date = new Date(seconds * 1000)
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`
}
