export type ConsoleApp = {
  appId: string
  name: string | null
  createdAt: number | null
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
  app_exists: 'あなたの app はすでにあります。',
  no_app: 'あなたの app がありません。',
  invalid_name: '名前は 1〜64 文字で、改行などの制御文字は使えません。',
  invalid_path: 'パスは「/」で区切った空でない名前で、255 文字までです。',
  no_scope: 'publish か subscribe の少なくとも一方を選んでください。',
  invalid_ttl: '期限は 1 分〜24 時間で指定してください。',
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

export const api = {
  me: () => request<{ app: ConsoleApp | null }>('GET', '/api/me'),
  createApp: (name: string) => request<{ app: ConsoleApp }>('POST', '/api/app', { name }),
  issueToken: (scope: TokenScope) => request<IssuedToken>('POST', '/api/tokens', scope),
  rotateSecret: () => request<{ app: ConsoleApp }>('POST', '/api/app/secret'),
  deleteApp: () => request<void>('DELETE', '/api/app')
}

export function formatUnixTime(seconds: number | null): string {
  return seconds === null ? '—' : new Date(seconds * 1000).toLocaleString('ja-JP')
}
