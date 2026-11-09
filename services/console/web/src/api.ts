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
  invalid_request: 'リクエストの形が正しくありません。'
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
  deleteApp: (appId: string) => request<void>('DELETE', appPath(appId))
}

export function formatUnixTime(seconds: number | null): string {
  if (seconds === null) {
    return '—'
  }
  const date = new Date(seconds * 1000)
  const pad = (value: number) => String(value).padStart(2, '0')
  return `${date.getFullYear()}/${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`
}
