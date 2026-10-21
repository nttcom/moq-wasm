import { useState, type FormEvent } from 'react'

import { api, describeError, formatUnixTime, type ConsoleApp } from './api'

type Props = {
  app: ConsoleApp | null
  onChange: (app: ConsoleApp | null) => void
}

export function AppPanel({ app, onChange }: Props) {
  return app ? <OwnedApp app={app} onDeleted={() => onChange(null)} /> : <CreateApp onCreated={onChange} />
}

function CreateApp({ onCreated }: { onCreated: (app: ConsoleApp) => void }) {
  const [name, setName] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setBusy(true)
    setError(null)
    try {
      onCreated((await api.createApp(name)).app)
    } catch (caught) {
      setError(describeError(caught))
    } finally {
      setBusy(false)
    }
  }

  return (
    <form onSubmit={submit} className="flex max-w-md flex-col gap-4">
      <div>
        <h2 className="text-lg font-semibold">app を作る</h2>
        <p className="mt-1 text-sm text-muted">app は、あなたが配信する場所です。1 人 1 つまで作れます。</p>
      </div>
      <label className="flex flex-col gap-1 text-sm">
        名前
        <input
          value={name}
          onChange={(event) => setName(event.target.value)}
          maxLength={64}
          placeholder="例：遠隔監視のカメラ"
          className="rounded-md border border-line px-3 py-2"
        />
      </label>
      {error && <p className="text-sm text-danger">{error}</p>}
      <button
        type="submit"
        disabled={busy}
        className="self-start rounded-md bg-accent px-4 py-2 text-sm font-semibold text-white disabled:opacity-50"
      >
        app を作る
      </button>
    </form>
  )
}

function OwnedApp({ app, onDeleted }: { app: ConsoleApp; onDeleted: () => void }) {
  const [confirming, setConfirming] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const remove = async () => {
    setError(null)
    try {
      await api.deleteApp()
      onDeleted()
    } catch (caught) {
      setError(describeError(caught))
    }
  }

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h2 className="text-lg font-semibold">あなたの app</h2>
        <dl className="mt-4 grid grid-cols-[7rem_1fr] gap-x-4 gap-y-2 text-sm">
          <dt className="text-muted">名前</dt>
          <dd>{app.name ?? '—'}</dd>
          <dt className="text-muted">appId</dt>
          <dd className="font-mono break-all">{app.appId}</dd>
          <dt className="text-muted">配信の場所</dt>
          <dd className="font-mono break-all">{app.appId}/…</dd>
          <dt className="text-muted">作った日時</dt>
          <dd>{formatUnixTime(app.createdAt)}</dd>
        </dl>
      </div>
      <section className="rounded-md border border-line p-4">
        <h3 className="text-sm font-semibold">app を削除する</h3>
        <p className="mt-1 text-sm text-muted">
          今までに発行したトークンは、すぐに使えなくなります。もう一度作ると appId は変わります。
        </p>
        {error && <p className="mt-2 text-sm text-danger">{error}</p>}
        {confirming ? (
          <div className="mt-3 flex gap-2">
            <button
              type="button"
              onClick={remove}
              className="rounded-md bg-danger px-4 py-2 text-sm font-semibold text-white"
            >
              削除する
            </button>
            <button
              type="button"
              onClick={() => setConfirming(false)}
              className="rounded-md border border-line px-4 py-2 text-sm"
            >
              やめる
            </button>
          </div>
        ) : (
          <button
            type="button"
            onClick={() => setConfirming(true)}
            className="mt-3 rounded-md border border-danger px-4 py-2 text-sm text-danger"
          >
            app を削除する…
          </button>
        )}
      </section>
    </div>
  )
}
