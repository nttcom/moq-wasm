import { useState, type FormEvent, type ReactNode } from 'react'

import { api, describeError, formatUnixTime, type ConsoleApp, type IssuedToken } from './api'
import type { SoonPage } from './navigation'
import { displayName } from './Sidebar'
import { Alert, Badge, Button, Card, Code, ErrorText } from './ui'

export function PageHeading({ title, subtitle, aside }: { title: string; subtitle?: string; aside?: ReactNode }) {
  return (
    <div className="mb-5.5 flex items-end justify-between gap-4">
      <div>
        <h1 className="text-[26px] font-bold text-strong">{title}</h1>
        {subtitle && <p className="mt-1 text-[13px] text-muted">{subtitle}</p>}
      </div>
      {aside}
    </div>
  )
}

export function EmptyState({ onCreate }: { onCreate: () => void }) {
  return (
    <div className="rounded-xl border border-dashed border-line-strong bg-white px-6 py-18 text-center">
      <h2 className="text-[22px] font-bold text-strong">最初の app を作成</h2>
      <p className="mx-auto mt-2 max-w-md text-sm leading-[1.7] text-muted">
        app は MoQ relay に配信・視聴するための単位です。作成すると署名用の secret が一度だけ表示されます。
      </p>
      <Button size="lg" icon="plus" onClick={onCreate} className="mt-6">
        app を作成
      </Button>
    </div>
  )
}

function Field({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-xs text-muted">{label}</p>
      {children}
    </div>
  )
}

function CodeBlock({ children }: { children: ReactNode }) {
  return <p className="rounded-md bg-surface px-3 py-2 font-mono text-[13px] break-all text-body">{children}</p>
}

type OverviewProps = {
  app: ConsoleApp
  relayUrl: string | null
  onShowAuth: () => void
  onShowTokens: () => void
  onDelete: () => void
}

export function OverviewPage({ app, relayUrl, onShowAuth, onShowTokens, onDelete }: OverviewProps) {
  return (
    <>
      <PageHeading title="概要" />
      <div className="grid grid-cols-[repeat(auto-fit,minmax(320px,1fr))] gap-4.5">
        <Card title="app の識別子">
          <div className="flex flex-col gap-4">
            <Field label="app 名">
              <p className="text-[15px] font-bold text-strong">{displayName(app)}</p>
            </Field>
            <Field label="app ID（JWT の appId に入れます）">
              <CodeBlock>{app.appId}</CodeBlock>
            </Field>
            <Field label="名前空間（トークンのパスはこの下）">
              <CodeBlock>{app.appId}/…</CodeBlock>
            </Field>
            {relayUrl && (
              <Field label="接続先 relay">
                <CodeBlock>{relayUrl}</CodeBlock>
              </Field>
            )}
            <p className="text-xs text-subtle">作成日 {formatUnixTime(app.createdAt)}</p>
          </div>
        </Card>
        <Card
          title="認証"
          aside={
            <Badge tone="success" dot>
              secret 有効
            </Badge>
          }
        >
          <div className="flex h-full flex-col gap-3">
            <Field label="secret 最終更新">
              <p className="text-[15px] font-bold text-strong">{formatUnixTime(app.rotatedAt ?? app.createdAt)}</p>
            </Field>
            <p className="text-[13px] leading-relaxed text-muted">
              本番はサーバーで secret を使って JWT に署名します。手元の検証用トークンは、コンソールから発行できます。
            </p>
            <div className="mt-auto flex flex-wrap gap-2 pt-2">
              <Button variant="secondary" size="sm" onClick={onShowAuth}>
                secret と署名
              </Button>
              <Button size="sm" onClick={onShowTokens}>
                検証用トークンを発行
              </Button>
            </div>
          </div>
        </Card>
      </div>
      <section className="mt-4.5 flex flex-wrap items-center justify-between gap-4 rounded-xl border border-danger-50 bg-white px-6 py-5 shadow-card">
        <div>
          <h2 className="text-[15px] font-bold text-danger">app を削除</h2>
          <p className="mt-1 text-[13px] text-muted">
            この app の secret
            とトークンは使えなくなります。接続中のクライアントは、次の接続かトークンの更新で断られます。元に戻せません。
          </p>
        </div>
        <Button variant="danger" size="sm" onClick={onDelete}>
          app を削除
        </Button>
      </section>
    </>
  )
}

export function AuthPage({ app, onRotate }: { app: ConsoleApp; onRotate: () => void }) {
  return (
    <>
      <PageHeading title="認証" subtitle="secret の管理と、サーバーでの JWT 署名" />
      <div className="flex flex-col gap-4.5">
        <Card
          title="署名用 secret"
          aside={
            <Badge tone="success" dot>
              有効
            </Badge>
          }
        >
          <div className="grid grid-cols-[repeat(auto-fit,minmax(280px,1fr))] gap-6">
            <div className="flex flex-col gap-3">
              <div className="flex items-center justify-between gap-3 rounded-lg border border-line bg-surface px-3.5 py-3">
                <span className="font-mono text-sm tracking-[0.08em] text-subtle">••••••••••••••••••••••••</span>
                <span className="shrink-0 text-xs text-subtle">再表示できません</span>
              </div>
              <p className="text-[13px] leading-relaxed text-muted">
                secret は、作成時と作り直し時に一度だけ表示されます。紛失・漏えいした場合は作り直してください。
              </p>
              <p className="text-[13px] text-muted">
                作成 {formatUnixTime(app.createdAt)}　最終更新 {formatUnixTime(app.rotatedAt ?? app.createdAt)}
              </p>
            </div>
            <div className="flex flex-col items-start gap-2 rounded-lg border border-danger/30 bg-danger-50 p-4">
              <h3 className="text-[13px] font-bold text-danger">secret を作り直す</h3>
              <p className="text-xs text-body">この app のすべてのトークンが、次の接続から使えなくなります。</p>
              <Button variant="danger" size="sm" onClick={onRotate}>
                作り直す
              </Button>
            </div>
          </div>
        </Card>
        <Card title="サーバーで JWT に署名する（本番向け）" aside={<Badge tone="info">HS256</Badge>}>
          <div className="grid grid-cols-[repeat(auto-fit,minmax(320px,1fr))] gap-6">
            <pre className="overflow-x-auto rounded-lg bg-navy-deep px-5 py-4.5 font-mono text-[12.5px] leading-[1.65] text-[#e6eef8]">
              {signingExample(app.appId)}
            </pre>
            <div className="flex flex-col gap-3 text-[13px] text-body">
              <p className="text-xs font-bold text-strong">トークンが持つもの</p>
              <Claim name="appId">この app の ID</Claim>
              <Claim name="publish / subscribe">
                配信・視聴を許す、app の下のパス（前方一致）。空文字で app 全体。書かなければ許さない
              </Claim>
              <Claim name="iat / exp">発行と期限の時刻（秒）。期限は発行から最大 24 時間</Claim>
              <p className="text-xs leading-relaxed text-muted">
                鍵は、secret の文字列そのものです（base64 として復号しません）。secret
                はサーバー側の環境変数などに置き、クライアントに渡さないでください。
              </p>
            </div>
          </div>
        </Card>
      </div>
    </>
  )
}

function Claim({ name, children }: { name: string; children: ReactNode }) {
  return (
    <div className="flex items-start gap-3">
      <Code>{name}</Code>
      <span className="leading-relaxed">{children}</span>
    </div>
  )
}

function signingExample(appId: string): string {
  return `import jwt from "jsonwebtoken";

const token = jwt.sign(
  {
    appId: "${appId}",
    publish: "live/room-1",   // 配信を許すパス
    subscribe: "live",        // 視聴を許すパス
  },
  process.env.MOQ_SECRET,     // コンソールで受け取った secret
  { algorithm: "HS256", expiresIn: "24h" }  // 最大 24 時間
);`
}

const ttlChoices = [1, 6, 12, 24]

type TokensProps = { app: ConsoleApp; onIssued: (issued: IssuedToken, summary: TokenSummary) => void }

export type TokenSummary = { operations: string; scope: string }

export function TokensPage({ app, onIssued }: TokensProps) {
  const [publish, setPublish] = useState(true)
  const [subscribe, setSubscribe] = useState(true)
  const [path, setPath] = useState('')
  const [hours, setHours] = useState(24)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      const issued = await api.issueToken(app.appId, {
        publish: publish ? path : undefined,
        subscribe: subscribe ? path : undefined,
        ttlSeconds: hours * 3600
      })
      onIssued(issued, {
        operations: [publish && '配信', subscribe && '視聴'].filter(Boolean).join('・'),
        scope: path === '' ? `${app.appId}/（app 全体）` : `${app.appId}/${path}`
      })
    } catch (caught) {
      setError(describeError(caught))
    } finally {
      setBusy(false)
    }
  }

  return (
    <>
      <PageHeading title="トークン発行" subtitle="手元の検証用トークンを発行します" />
      <div className="flex max-w-2xl flex-col gap-4.5">
        <Alert tone="info" title="手元の検証向け">
          ここで発行するトークンは検証用です。本番ではサーバーで secret
          を使って署名してください。発行したトークンは一度だけ表示され、期限まで取り消せません。
        </Alert>
        <Card title="トークンを発行">
          <form onSubmit={submit} className="flex flex-col gap-5">
            <fieldset className="flex flex-col gap-2">
              <legend className="mb-1 text-[13px] font-bold text-strong">許可する操作</legend>
              <Checkbox checked={publish} onChange={setPublish} label="配信（publish）" />
              <Checkbox checked={subscribe} onChange={setSubscribe} label="視聴（subscribe）" />
            </fieldset>
            <label className="flex flex-col gap-1.5">
              <span className="text-[13px] font-bold text-strong">パス（範囲）</span>
              <input
                value={path}
                onChange={(event) => setPath(event.target.value)}
                maxLength={255}
                placeholder="live/room-1"
                className="rounded-lg border border-line px-3 py-2 font-mono text-sm focus-visible:outline-2 focus-visible:outline-primary"
              />
              <span className="text-xs leading-relaxed text-muted">
                <span className="font-mono">{app.appId}/</span> の下のパスです。この下すべてが対象になります。空にすると
                app 全体です。
              </span>
            </label>
            <label className="flex flex-col gap-1.5">
              <span className="text-[13px] font-bold text-strong">期限</span>
              <select
                value={hours}
                onChange={(event) => setHours(Number(event.target.value))}
                className="rounded-lg border border-line bg-white px-3 py-2 text-sm focus-visible:outline-2 focus-visible:outline-primary"
              >
                {ttlChoices.map((choice) => (
                  <option key={choice} value={choice}>
                    {choice} 時間{choice === 24 ? '（最大）' : ''}
                  </option>
                ))}
              </select>
              <span className="text-xs text-muted">最大 24 時間。期限前の取り消しはできません。</span>
            </label>
            <ErrorText message={error} />
            <Button type="submit" block disabled={busy || (!publish && !subscribe)}>
              トークンを発行
            </Button>
          </form>
        </Card>
      </div>
    </>
  )
}

export function Checkbox({
  checked,
  onChange,
  label
}: {
  checked: boolean
  onChange: (checked: boolean) => void
  label: string
}) {
  return (
    <label className="flex items-center gap-2.5 text-sm text-body">
      <input
        type="checkbox"
        checked={checked}
        onChange={(event) => onChange(event.target.checked)}
        className="size-4 accent-primary"
      />
      {label}
    </label>
  )
}

type AppsProps = { apps: ConsoleApp[]; currentId: string | null; onOpen: (appId: string) => void; onCreate: () => void }

export function AppsPage({ apps, currentId, onOpen, onCreate }: AppsProps) {
  return (
    <>
      <PageHeading
        title="すべての app"
        subtitle={`${apps.length} 件 · 左上の切り替えからも選べます`}
        aside={
          <Button size="sm" icon="plus" onClick={onCreate}>
            app を作成
          </Button>
        }
      />
      {apps.length === 0 ? (
        <EmptyState onCreate={onCreate} />
      ) : (
        <div className="overflow-x-auto rounded-xl border border-line bg-white shadow-card">
          <table className="w-full border-collapse text-left text-sm">
            <thead>
              <tr className="border-b border-line text-[11px] font-bold tracking-[0.06em] whitespace-nowrap text-subtle uppercase">
                <th className="px-5 py-3.5">app</th>
                <th className="px-5 py-3.5">app ID</th>
                <th className="px-5 py-3.5">作成日</th>
                <th className="px-5 py-3.5">secret 更新</th>
                <th className="px-5 py-3.5">
                  <span className="sr-only">開く</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {apps.map((app) => {
                const selected = app.appId === currentId
                return (
                  <tr
                    key={app.appId}
                    className={`border-b border-divider last:border-b-0 ${selected ? 'bg-brand-50' : ''}`}
                  >
                    <td className="px-5 py-4 font-bold text-strong">
                      {displayName(app)}
                      {selected && (
                        <span className="ml-2 rounded-full bg-white px-2 py-0.5 text-[10px] font-bold text-brand-700">
                          選択中
                        </span>
                      )}
                    </td>
                    <td className="px-5 py-4 whitespace-nowrap">
                      <Code>{app.appId}</Code>
                    </td>
                    <td className="px-5 py-4 whitespace-nowrap text-muted">{formatUnixTime(app.createdAt)}</td>
                    <td className="px-5 py-4 whitespace-nowrap text-muted">
                      {formatUnixTime(app.rotatedAt ?? app.createdAt)}
                    </td>
                    <td className="px-5 py-4 text-right">
                      <Button variant="ghost" size="sm" onClick={() => onOpen(app.appId)}>
                        開く →
                      </Button>
                    </td>
                  </tr>
                )
              })}
            </tbody>
          </table>
        </div>
      )}
    </>
  )
}

const soonContent: Record<SoonPage, { title: string; description: string; items: string[] }> = {
  analytics: {
    title: 'Analytics',
    description: 'この app の配信・視聴の実績を、期間を区切って振り返ります。',
    items: ['配信本数・視聴セッション数の推移', 'パス（名前空間）別の利用量', '期間を指定したレポートの出力']
  },
  observability: {
    title: 'Observability',
    description: 'この app の relay と接続が、いま正常に動いているかを監視します。',
    items: ['relay の稼働状況とレイテンシ', '接続中のセッション一覧', 'エラー・切断の履歴とアラート']
  },
  media: {
    title: 'メディア処理',
    description: 'この app の配信に対する録画・トランスコードなどの後処理を設定します。',
    items: ['配信の録画と保存先', 'トランスコードのプロファイル', '処理ジョブの履歴']
  },
  billing: {
    title: 'Billing',
    description: 'アカウント全体の利用料金を確認します。app ごとの内訳も見られます。',
    items: ['今月の利用額と app 別の内訳', '請求履歴', '支払い方法']
  }
}

type SoonProps = { page: SoonPage; app: ConsoleApp | null; onShowOverview: () => void }

export function SoonPageView({ page, app, onShowOverview }: SoonProps) {
  const content = soonContent[page]
  return (
    <div className="grid grid-cols-[repeat(auto-fit,minmax(260px,1fr))] gap-7 rounded-xl border border-line bg-white px-8 py-7 shadow-card">
      <div className="flex flex-col gap-3">
        <div className="flex flex-wrap items-center gap-2">
          <h1 className="text-[22px] font-bold text-strong">{content.title}</h1>
          <Badge tone="outline">準備中</Badge>
          {page !== 'billing' && app && <Badge tone="info">app: {displayName(app)}</Badge>}
        </div>
        <p className="text-sm text-body">{content.description}</p>
        <p className="mt-2 text-xs font-bold text-strong">予定している内容</p>
        <ul className="flex flex-col gap-2">
          {content.items.map((item) => (
            <li key={item} className="flex items-center gap-2.5 text-sm text-body">
              <span className="size-1.5 rounded-full bg-brand-300" />
              {item}
            </li>
          ))}
        </ul>
      </div>
      <div className="flex flex-col items-start gap-2 self-start rounded-[10px] bg-brand-50 p-5">
        <p className="text-[13px] font-bold text-navy">今できること</p>
        <p className="text-[13px] text-body">app の作成、secret の管理、検証用トークンの発行は利用できます。</p>
        <Button variant="secondary" size="sm" onClick={onShowOverview} className="mt-1">
          app の概要へ
        </Button>
      </div>
    </div>
  )
}
