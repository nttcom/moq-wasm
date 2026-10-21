import { useEffect, useState } from 'react'

import { api, describeError, type AppWithSecret, type ConsoleApp, type IssuedToken, type Me } from './api'
import { CreateAppModal, DeleteAppModal, RotateSecretModal, SecretModal, TokenModal } from './modals'
import { isAppPage, pageTitles, type Page, type SoonPage } from './navigation'
import { AppsPage, AuthPage, EmptyState, OverviewPage, SoonPageView, TokensPage, type TokenSummary } from './pages'
import { displayName, Sidebar } from './Sidebar'

const docsUrl = 'https://github.com/nttcom/moq-wasm/blob/master/crates/console/README.md'

type Loaded =
  { state: 'loading' } | { state: 'failed'; message: string } | { state: 'ready'; me: Me; apps: ConsoleApp[] }

type Dialog =
  | { kind: 'create' }
  | { kind: 'secret'; app: ConsoleApp; secret: string; rotated: boolean }
  | { kind: 'rotate'; app: ConsoleApp }
  | { kind: 'delete'; app: ConsoleApp }
  | { kind: 'token'; issued: IssuedToken; summary: TokenSummary }

export function App() {
  const [loaded, setLoaded] = useState<Loaded>({ state: 'loading' })
  const [currentId, setCurrentId] = useState<string | null>(null)
  const [page, setPage] = useState<Page>('overview')
  const [dialog, setDialog] = useState<Dialog | null>(null)

  useEffect(() => {
    Promise.all([api.me(), api.listApps()])
      .then(([me, { apps }]) => {
        setLoaded({ state: 'ready', me, apps })
        setCurrentId(apps[0]?.appId ?? null)
      })
      .catch((error: unknown) => setLoaded({ state: 'failed', message: describeError(error) }))
  }, [])

  if (loaded.state !== 'ready') {
    return (
      <main className="grid min-h-screen place-items-center bg-surface text-sm">
        {loaded.state === 'loading' ? (
          <p className="text-muted">読み込んでいます…</p>
        ) : (
          <p className="text-danger">{loaded.message}</p>
        )}
      </main>
    )
  }

  const { me, apps } = loaded
  const current = apps.find((app) => app.appId === currentId) ?? null
  const setApps = (next: ConsoleApp[]) => setLoaded({ state: 'ready', me, apps: next })
  const replaceApp = (app: ConsoleApp) =>
    setApps(apps.map((existing) => (existing.appId === app.appId ? app : existing)))

  const created = ({ app, secret }: AppWithSecret) => {
    setApps([...apps, app])
    setCurrentId(app.appId)
    setPage('overview')
    setDialog({ kind: 'secret', app, secret, rotated: false })
  }
  const rotated = ({ app, secret }: AppWithSecret) => {
    replaceApp(app)
    setDialog({ kind: 'secret', app, secret, rotated: true })
  }
  const deleted = (appId: string) => {
    const remaining = apps.filter((app) => app.appId !== appId)
    setApps(remaining)
    setCurrentId(remaining[0]?.appId ?? null)
    setPage(remaining.length > 0 ? 'apps' : 'overview')
    setDialog(null)
  }
  const selectApp = (appId: string) => {
    setCurrentId(appId)
    if (!isAppPage(page)) {
      setPage('overview')
    }
  }
  const openApp = (appId: string) => {
    setCurrentId(appId)
    setPage('overview')
  }
  const startCreate = () => setDialog({ kind: 'create' })
  const scoped = isAppPage(page)

  return (
    <div className="grid min-h-screen grid-cols-[248px_1fr] bg-surface font-sans text-body">
      <div className="sticky top-0 h-screen">
        <Sidebar
          apps={apps}
          current={current}
          page={page}
          email={me.email}
          onSelectApp={selectApp}
          onNavigate={setPage}
          onCreateApp={startCreate}
        />
      </div>
      <div className="flex min-w-0 flex-col">
        <header className="flex h-16 shrink-0 items-center justify-between border-b border-line bg-white px-7">
          <nav aria-label="現在の場所" className="flex items-center gap-2 text-sm text-muted">
            <span>MoQ Console</span>
            {scoped && current && (
              <>
                <span aria-hidden="true">›</span>
                <span className="font-mono text-[13px]">{displayName(current)}</span>
              </>
            )}
            <span aria-hidden="true">›</span>
            <span className="font-bold text-strong">{pageTitles[page]}</span>
          </nav>
          <a href={docsUrl} target="_blank" rel="noreferrer" className="text-[13px] text-primary hover:underline">
            ドキュメント
          </a>
        </header>
        <main key={`${page}-${currentId}`} className="animate-fade overflow-y-auto p-7">
          <PageBody
            page={page}
            apps={apps}
            current={current}
            relayUrl={me.relayUrl}
            onNavigate={setPage}
            onOpenApp={openApp}
            onCreate={startCreate}
            onDialog={setDialog}
          />
        </main>
      </div>
      {dialog?.kind === 'create' && <CreateAppModal onCreated={created} onClose={() => setDialog(null)} />}
      {dialog?.kind === 'secret' && (
        <SecretModal app={dialog.app} secret={dialog.secret} rotated={dialog.rotated} onClose={() => setDialog(null)} />
      )}
      {dialog?.kind === 'rotate' && (
        <RotateSecretModal app={dialog.app} onDone={rotated} onClose={() => setDialog(null)} />
      )}
      {dialog?.kind === 'delete' && (
        <DeleteAppModal app={dialog.app} onDeleted={() => deleted(dialog.app.appId)} onClose={() => setDialog(null)} />
      )}
      {dialog?.kind === 'token' && (
        <TokenModal issued={dialog.issued} summary={dialog.summary} onClose={() => setDialog(null)} />
      )}
    </div>
  )
}

type PageBodyProps = {
  page: Page
  apps: ConsoleApp[]
  current: ConsoleApp | null
  relayUrl: string | null
  onNavigate: (page: Page) => void
  onOpenApp: (appId: string) => void
  onCreate: () => void
  onDialog: (dialog: Dialog) => void
}

function PageBody({ page, apps, current, relayUrl, onNavigate, onOpenApp, onCreate, onDialog }: PageBodyProps) {
  if (page === 'apps') {
    return <AppsPage apps={apps} currentId={current?.appId ?? null} onOpen={onOpenApp} onCreate={onCreate} />
  }
  if (isSoonPage(page)) {
    return <SoonPageView page={page} app={current} onShowOverview={() => onNavigate('overview')} />
  }
  if (!current) {
    return <EmptyState onCreate={onCreate} />
  }
  switch (page) {
    case 'auth':
      return <AuthPage app={current} onRotate={() => onDialog({ kind: 'rotate', app: current })} />
    case 'tokens':
      return <TokensPage app={current} onIssued={(issued, summary) => onDialog({ kind: 'token', issued, summary })} />
    default:
      return (
        <OverviewPage
          app={current}
          relayUrl={relayUrl}
          onShowAuth={() => onNavigate('auth')}
          onShowTokens={() => onNavigate('tokens')}
          onDelete={() => onDialog({ kind: 'delete', app: current })}
        />
      )
  }
}

function isSoonPage(page: Page): page is SoonPage {
  return page === 'analytics' || page === 'observability' || page === 'media' || page === 'billing'
}
