import { useState, type FormEvent } from 'react'

import { api, describeError, formatUnixTime, type ConsoleApp, type IssuedToken } from './api'

const presets = [
  { label: '1 時間', minutes: 60 },
  { label: '12 時間', minutes: 12 * 60 },
  { label: '24 時間', minutes: 24 * 60 }
]

type PathField = { enabled: boolean; path: string }

export function TokenPanel({ app }: { app: ConsoleApp }) {
  const [publish, setPublish] = useState<PathField>({ enabled: true, path: '' })
  const [subscribe, setSubscribe] = useState<PathField>({ enabled: false, path: '' })
  const [minutes, setMinutes] = useState(60)
  const [issued, setIssued] = useState<IssuedToken | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)

  const submit = async (event: FormEvent) => {
    event.preventDefault()
    setError(null)
    setIssued(null)
    setCopied(false)
    try {
      setIssued(
        await api.issueToken({
          publish: publish.enabled ? publish.path : undefined,
          subscribe: subscribe.enabled ? subscribe.path : undefined,
          ttlSeconds: minutes * 60
        })
      )
    } catch (caught) {
      setError(describeError(caught))
    }
  }

  const copy = async () => {
    if (issued) {
      await navigator.clipboard.writeText(issued.token)
      setCopied(true)
    }
  }

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h2 className="text-lg font-semibold">トークン</h2>
        <p className="mt-1 text-sm text-muted">範囲と期限を選んで、JWT を発行します。何枚でも発行できます。</p>
      </div>
      <form onSubmit={submit} className="flex flex-col gap-5">
        <PathInput label="publish" field={publish} onChange={setPublish} appId={app.appId} verb="配信できる" />
        <PathInput label="subscribe" field={subscribe} onChange={setSubscribe} appId={app.appId} verb="視聴できる" />
        <fieldset className="flex flex-col gap-2 text-sm">
          <legend className="font-semibold">期限</legend>
          <div className="flex flex-wrap items-center gap-2">
            {presets.map((preset) => (
              <button
                key={preset.minutes}
                type="button"
                aria-pressed={minutes === preset.minutes}
                onClick={() => setMinutes(preset.minutes)}
                className="rounded-md border border-line px-3 py-1.5 aria-pressed:border-accent aria-pressed:text-accent"
              >
                {preset.label}
              </button>
            ))}
            <label className="flex items-center gap-2">
              <input
                type="number"
                min={1}
                max={24 * 60}
                value={minutes}
                onChange={(event) => setMinutes(Number(event.target.value))}
                className="w-24 rounded-md border border-line px-2 py-1.5"
              />
              分
            </label>
          </div>
        </fieldset>
        {error && <p className="text-sm text-danger">{error}</p>}
        <button type="submit" className="self-start rounded-md bg-accent px-4 py-2 text-sm font-semibold text-white">
          発行する
        </button>
      </form>
      {issued && (
        <section className="flex flex-col gap-2 rounded-md border border-line bg-wash p-4">
          <h3 className="text-sm font-semibold">発行した JWT</h3>
          <textarea
            readOnly
            value={issued.token}
            rows={4}
            className="w-full resize-none rounded-md border border-line bg-white p-2 font-mono text-xs break-all"
          />
          <div className="flex flex-wrap items-center gap-3 text-sm">
            <button type="button" onClick={copy} className="rounded-md border border-line bg-white px-3 py-1.5">
              {copied ? 'コピーしました' : 'コピー'}
            </button>
            <span className="text-muted">期限：{formatUnixTime(issued.expiresAt)}</span>
          </div>
          <p className="text-sm text-muted">
            この JWT は、ここでしか表示されません。期限まで取り消せません。漏れたときは secret を作り直します。
          </p>
        </section>
      )}
    </div>
  )
}

type PathInputProps = {
  label: string
  field: PathField
  onChange: (field: PathField) => void
  appId: string
  verb: string
}

function PathInput({ label, field, onChange, appId, verb }: PathInputProps) {
  const scope = field.path === '' ? `${appId}/ の下すべて` : `${appId}/${field.path} の下`
  return (
    <fieldset className="flex flex-col gap-2 text-sm">
      <label className="flex items-center gap-2 font-semibold">
        <input
          type="checkbox"
          checked={field.enabled}
          onChange={(event) => onChange({ ...field, enabled: event.target.checked })}
        />
        {label} を許可する
      </label>
      {field.enabled && (
        <div className="flex flex-col gap-1 pl-6">
          <input
            value={field.path}
            onChange={(event) => onChange({ ...field, path: event.target.value })}
            maxLength={255}
            placeholder="空にすると app の下すべて（例：site1/cam1）"
            className="rounded-md border border-line px-3 py-2 font-mono"
          />
          <span className="font-mono text-xs break-all text-muted">
            → {scope}に{verb}
          </span>
        </div>
      )}
    </fieldset>
  )
}
