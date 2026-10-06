import { useEffect, useState } from 'react'
import { type NamespaceSummary, type PublishedNamespaces, fetchNamespaces } from './api'
import { appIdOf, relativeNamespace } from './topology'

const LIVE_REFRESH_MS = 10_000
const LIVE_TOLERANCE_MS = 3_000

interface Props {
  spanMs: number
  toMs: number
  live: boolean
  appId: string
  onPick: (summary: NamespaceSummary, stillPublished: boolean) => void
  onClose: () => void
}

const shortAppId = (appId: string) => (appId.length > 12 ? `${appId.slice(0, 8)}…` : appId)

function timeOf(ms: number, withDate: boolean): string {
  return new Date(ms).toLocaleString([], {
    ...(withDate ? { month: '2-digit', day: '2-digit' } : {}),
    hour: '2-digit',
    minute: '2-digit'
  })
}

export function NamespacePanel({ spanMs, toMs, live, appId, onPick, onClose }: Props) {
  const [result, setResult] = useState<PublishedNamespaces | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [search, setSearch] = useState('')
  const queryToMs = live ? Math.floor(toMs / LIVE_REFRESH_MS) * LIVE_REFRESH_MS : toMs
  const queryFromMs = queryToMs - spanMs

  useEffect(() => {
    let cancelled = false
    fetchNamespaces(queryFromMs, queryToMs, appId)
      .then((next) => {
        if (cancelled) return
        setResult(next)
        setError(null)
      })
      .catch((reason: unknown) => {
        if (!cancelled) setError(String(reason))
      })
    return () => {
      cancelled = true
    }
  }, [queryFromMs, queryToMs, appId])

  const spansDays = spanMs > 86_400_000
  const needle = search.trim().toLowerCase()
  const rows = (result?.namespaces ?? []).filter((summary) => summary.namespace.toLowerCase().includes(needle))
  const stillPublished = (summary: NamespaceSummary) => live && summary.last_seen_ms >= queryToMs - LIVE_TOLERANCE_MS

  return (
    <section className="namespace-panel" aria-label="Published Track Namespaces">
      <div className="panel-head">
        <b>Namespaces</b>
        <button className="close" aria-label="Close namespaces" onClick={onClose}>
          ×
        </button>
      </div>
      <div className="panel-counts">
        {result
          ? `${result.namespaces.length} namespaces · ${result.subscriptions} subscriptions · ${result.clients} clients`
          : 'loading'}
        {error && <span className="error"> {error}</span>}
      </div>
      <input
        className="panel-search"
        placeholder="search, e.g. room1"
        value={search}
        onChange={(event) => setSearch(event.target.value)}
      />
      <div className="panel-rows">
        {rows.map((summary) => {
          const published = stillPublished(summary)
          return (
            <button key={summary.namespace} className="namespace-row" onClick={() => onPick(summary, published)}>
              <span className="namespace-name">{relativeNamespace(summary.namespace) || '(app root)'}</span>
              <span className="namespace-app">{shortAppId(appIdOf(summary.namespace))}</span>
              <span className="namespace-tracks">{summary.tracks.join(', ')}</span>
              <span className="namespace-meta">
                {summary.relays.join(', ')} · {summary.subscriptions} subscriptions
              </span>
              <span className={`namespace-time${published ? ' live-now' : ''}`}>
                {timeOf(summary.first_seen_ms, spansDays)} –{' '}
                {published ? 'Live' : timeOf(summary.last_seen_ms, spansDays)}
              </span>
            </button>
          )
        })}
        {result && rows.length === 0 && <div className="note">No namespace was published in this range</div>}
      </div>
    </section>
  )
}
