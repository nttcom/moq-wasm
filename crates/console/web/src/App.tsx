import { useEffect, useState } from 'react'

import { api, describeError, type ConsoleApp } from './api'
import { AppPanel } from './AppPanel'
import { SecretPanel } from './SecretPanel'
import { TokenPanel } from './TokenPanel'

type Tab = 'app' | 'token' | 'secret'

const tabs: { id: Tab; label: string }[] = [
  { id: 'app', label: 'app' },
  { id: 'token', label: 'トークン' },
  { id: 'secret', label: 'secret' }
]

type Loaded = { state: 'loading' } | { state: 'failed'; message: string } | { state: 'ready'; app: ConsoleApp | null }

export function App() {
  const [loaded, setLoaded] = useState<Loaded>({ state: 'loading' })
  const [tab, setTab] = useState<Tab>('app')

  useEffect(() => {
    api
      .me()
      .then(({ app }) => setLoaded({ state: 'ready', app }))
      .catch((error: unknown) => setLoaded({ state: 'failed', message: describeError(error) }))
  }, [])

  const app = loaded.state === 'ready' ? loaded.app : null
  const setApp = (next: ConsoleApp | null) => {
    setLoaded({ state: 'ready', app: next })
    if (next === null) {
      setTab('app')
    }
  }

  return (
    <div className="min-h-screen bg-wash font-sans text-ink">
      <header className="flex items-center gap-4 border-b border-line bg-white px-6 py-3">
        <h1 className="text-base font-semibold">MoQ コンソール</h1>
        {app && <span className="text-sm text-muted">{app.name ?? app.appId}</span>}
      </header>
      <div className="mx-auto grid max-w-5xl grid-cols-[10rem_1fr] gap-6 px-6 py-8">
        <nav aria-label="画面" className="flex flex-col gap-1">
          {tabs.map(({ id, label }) => {
            const disabled = id !== 'app' && app === null
            return (
              <button
                key={id}
                type="button"
                disabled={disabled}
                aria-current={tab === id ? 'page' : undefined}
                onClick={() => setTab(id)}
                className="rounded-md px-3 py-2 text-left text-sm aria-[current=page]:bg-white aria-[current=page]:font-semibold aria-[current=page]:shadow-sm disabled:cursor-not-allowed disabled:opacity-40"
              >
                {label}
              </button>
            )
          })}
        </nav>
        <main className="rounded-lg border border-line bg-white p-6">
          {loaded.state === 'loading' && <p className="text-muted">読み込んでいます…</p>}
          {loaded.state === 'failed' && <p className="text-danger">{loaded.message}</p>}
          {loaded.state === 'ready' && tab === 'app' && <AppPanel app={app} onChange={setApp} />}
          {loaded.state === 'ready' && tab === 'token' && app && <TokenPanel app={app} />}
          {loaded.state === 'ready' && tab === 'secret' && app && <SecretPanel onRotated={setApp} />}
        </main>
      </div>
    </div>
  )
}
