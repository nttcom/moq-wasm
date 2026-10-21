import { useState } from 'react'

import { api, describeError, type ConsoleApp } from './api'
import { SecretReveal } from './SecretReveal'

export function SecretPanel({ onRotated }: { onRotated: (app: ConsoleApp) => void }) {
  const [confirming, setConfirming] = useState(false)
  const [secret, setSecret] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const rotate = async () => {
    setError(null)
    try {
      const rotated = await api.rotateSecret()
      onRotated(rotated.app)
      setSecret(rotated.secret)
      setConfirming(false)
    } catch (caught) {
      setError(describeError(caught))
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h2 className="text-lg font-semibold">secret</h2>
        <p className="mt-1 text-sm text-muted">
          secret か JWT が漏れたときに、secret を作り直します。appId は変わりません。
        </p>
      </div>
      <section className="rounded-md border border-line p-4">
        <h3 className="text-sm font-semibold">secret を作り直す</h3>
        <p className="mt-1 text-sm text-muted">
          今までに発行した JWT は、すぐに使えなくなります。配信中のクライアントは、次に接続するときから断られます。
        </p>
        {error && <p className="mt-2 text-sm text-danger">{error}</p>}
        {confirming ? (
          <div className="mt-3 flex gap-2">
            <button
              type="button"
              onClick={rotate}
              className="rounded-md bg-danger px-4 py-2 text-sm font-semibold text-white"
            >
              作り直す
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
            onClick={() => {
              setSecret(null)
              setConfirming(true)
            }}
            className="mt-3 rounded-md border border-danger px-4 py-2 text-sm text-danger"
          >
            secret を作り直す…
          </button>
        )}
      </section>
      {secret && <SecretReveal secret={secret} />}
    </div>
  )
}
