import { useEffect, useRef, useState } from 'react'

import { api, describeError, formatBytes, type ConsoleApp, type Usage } from './api'
import { PageHeading } from './pages'
import { displayName } from './Sidebar'
import { Badge, Card, ErrorText } from './ui'

const RANGES = [
  { label: '1 時間', ms: 3_600_000 },
  { label: '6 時間', ms: 6 * 3_600_000 },
  { label: '1 日', ms: 86_400_000 },
  { label: '7 日', ms: 7 * 86_400_000 }
] as const
const POINTS = 120
const UP = 'var(--color-series-up)'
const DOWN = 'var(--color-series-down)'
/** Upper bounds of the session distribution's buckets, in bytes; the last bucket is open. */
const SESSION_BUCKETS = [1e6, 1e7, 1e8, 1e9]

function formatTime(ms: number, spanMs: number): string {
  return new Date(ms).toLocaleString([], {
    ...(spanMs > 86_400_000 ? { month: 'numeric', day: 'numeric' } : {}),
    hour: '2-digit',
    minute: '2-digit'
  })
}

/** The app's traffic through the relays, read from the observability server for the chosen range. */
export function UsagePage({ app }: { app: ConsoleApp }) {
  const [rangeMs, setRangeMs] = useState<number>(RANGES[0].ms)
  const [usage, setUsage] = useState<{ data: Usage; fromMs: number; toMs: number } | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let stopped = false
    const toMs = Date.now()
    const fromMs = toMs - rangeMs
    setError(null)
    api
      .usage(app.appId, fromMs, toMs, POINTS)
      .then((data) => {
        if (!stopped) setUsage({ data, fromMs, toMs })
      })
      .catch((reason: unknown) => {
        if (!stopped) setError(describeError(reason))
      })
    return () => {
      stopped = true
    }
  }, [app.appId, rangeMs])

  return (
    <>
      <PageHeading
        title="Observability"
        subtitle="この app の client が relay とやり取りした通信量（relay 同士の転送は含みません）"
        aside={
          <div className="flex items-center gap-1 rounded-lg border border-line bg-white p-1" role="group">
            {RANGES.map((range) => (
              <button
                key={range.ms}
                onClick={() => setRangeMs(range.ms)}
                aria-pressed={range.ms === rangeMs}
                className={`rounded-md px-3 py-1 text-[13px] ${
                  range.ms === rangeMs ? 'bg-primary text-white' : 'text-body hover:bg-brand-50'
                }`}
              >
                {range.label}
              </button>
            ))}
          </div>
        }
      />
      <div className="mb-4 flex items-center gap-2">
        <Badge tone="info">app: {displayName(app)}</Badge>
      </div>
      <ErrorText message={error} />
      {usage && <UsageBody usage={usage.data} fromMs={usage.fromMs} toMs={usage.toMs} />}
    </>
  )
}

function UsageBody({ usage, fromMs, toMs }: { usage: Usage; fromMs: number; toMs: number }) {
  const sent = usage.relays.reduce((sum, relay) => sum + relay.sent_bytes, 0)
  const received = usage.relays.reduce((sum, relay) => sum + relay.received_bytes, 0)
  const sessions = usage.relays.reduce((sum, relay) => sum + relay.sessions, 0)
  const peak = Math.max(0, ...usage.traffic.series.sessions.map((value) => value ?? 0))
  return (
    <div className="flex flex-col gap-4.5">
      <div className="grid grid-cols-[repeat(auto-fit,minmax(180px,1fr))] gap-4.5">
        <StatTile label="下り（relay → 視聴者）" value={formatBytes(sent)} />
        <StatTile label="上り（配信者 → relay）" value={formatBytes(received)} />
        <StatTile label="セッション" value={sessions.toLocaleString()} />
        <StatTile label="最大同時セッション" value={peak.toLocaleString()} />
      </div>
      <Card title="通信速度の推移">
        <TrafficChart traffic={usage.traffic} fromMs={fromMs} toMs={toMs} />
      </Card>
      <div className="grid grid-cols-[repeat(auto-fit,minmax(380px,1fr))] gap-4.5">
        <Card title="relay 別の通信量">
          <RelayBars relays={usage.relays} />
        </Card>
        <Card title="セッションごとの通信量の分布">
          <SessionHistogram sessions={usage.sessions} />
        </Card>
      </div>
      <Card title="Track Namespace 別の通信量（上位 20）">
        <NamespaceTable namespaces={usage.namespaces} />
      </Card>
    </div>
  )
}

function Empty({ text }: { text: string }) {
  return <p className="py-8 text-center text-sm text-muted">{text}</p>
}

function StatTile({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-xl border border-line bg-white px-5 py-4 shadow-card">
      <p className="text-xs text-muted">{label}</p>
      <p className="mt-1 text-[24px] font-bold text-strong tabular-nums">{value}</p>
    </div>
  )
}

function Legend({ unit }: { unit?: string }) {
  return (
    <div className="flex items-center gap-4 text-xs text-body">
      {unit && <span className="text-muted">{unit}</span>}
      <span className="flex items-center gap-1.5">
        <span className="h-0.5 w-4 rounded-full" style={{ background: UP }} />
        上り
      </span>
      <span className="flex items-center gap-1.5">
        <span className="h-0.5 w-4 rounded-full" style={{ background: DOWN }} />
        下り
      </span>
    </div>
  )
}

function useWidth<T extends HTMLElement>(): [React.RefObject<T | null>, number] {
  const ref = useRef<T>(null)
  const [width, setWidth] = useState(0)
  useEffect(() => {
    const element = ref.current
    if (!element) return
    const observer = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width))
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  return [ref, width]
}

/** A clean step of 1, 2 or 5 times a power of ten at or above `max / 4`. */
function niceStep(max: number): number {
  const rough = Math.max(max, 1e-9) / 4
  const power = 10 ** Math.floor(Math.log10(rough))
  return [1, 2, 5, 10].map((factor) => factor * power).find((step) => step >= rough) ?? power * 10
}

const formatMbps = (value: number) => String(Number(value.toFixed(value >= 10 ? 0 : value >= 1 ? 1 : 2)))

function TrafficChart({ traffic, fromMs, toMs }: { traffic: Usage['traffic']; fromMs: number; toMs: number }) {
  const [ref, width] = useWidth<HTMLDivElement>()
  const [hover, setHover] = useState<number | null>(null)
  const height = 220
  const margin = { top: 12, right: 48, bottom: 24, left: 48 }
  const up = traffic.series.ingress_mbps ?? []
  const down = traffic.series.egress_mbps ?? []
  const values = [...up, ...down].filter((value): value is number => value !== null)
  if (values.length === 0) {
    return <Empty text="この期間の通信はありません。" />
  }
  const step = niceStep(Math.max(...values))
  const top = Math.ceil(Math.max(...values) / step) * step || step
  const plotWidth = Math.max(0, width - margin.left - margin.right)
  const plotHeight = height - margin.top - margin.bottom
  const x = (ms: number) => margin.left + ((ms - fromMs) / (toMs - fromMs)) * plotWidth
  const y = (value: number) => margin.top + plotHeight - (value / top) * plotHeight
  const path = (series: (number | null)[]) =>
    series
      .map((value, index) =>
        value === null
          ? ''
          : `${index === 0 || series[index - 1] === null ? 'M' : 'L'}${x(traffic.t[index])},${y(value)}`
      )
      .join(' ')
  const ticks = Array.from({ length: Math.round(top / step) + 1 }, (_, index) => index * step)
  const timeTicks = Array.from({ length: 5 }, (_, index) => fromMs + ((toMs - fromMs) * index) / 4)
  const onMove = (event: React.PointerEvent<SVGRectElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect()
    const ms = fromMs + ((event.clientX - bounds.left) / bounds.width) * (toMs - fromMs)
    let nearest = 0
    traffic.t.forEach((t, index) => {
      if (Math.abs(t - ms) < Math.abs(traffic.t[nearest] - ms)) nearest = index
    })
    setHover(nearest)
  }
  return (
    <div className="flex flex-col gap-3">
      <Legend unit="Mbps" />
      <div ref={ref} className="relative">
        {width > 0 && (
          <svg width={width} height={height} role="img" aria-label="上りと下りの通信速度（Mbps）の推移">
            {ticks.map((tick) => (
              <g key={tick}>
                <line x1={margin.left} x2={width - margin.right} y1={y(tick)} y2={y(tick)} stroke="#eceef1" />
                <text x={margin.left - 8} y={y(tick)} dy="0.32em" textAnchor="end" className="fill-muted text-[11px]">
                  {formatMbps(tick)}
                </text>
              </g>
            ))}
            {timeTicks.map((ms) => (
              <text key={ms} x={x(ms)} y={height - 6} textAnchor="middle" className="fill-muted text-[11px]">
                {formatTime(ms, toMs - fromMs)}
              </text>
            ))}
            {[
              { series: up, color: UP },
              { series: down, color: DOWN }
            ].map(({ series, color }) => (
              <path
                key={color}
                d={path(series)}
                fill="none"
                stroke={color}
                strokeWidth={2}
                strokeLinejoin="round"
                strokeLinecap="round"
              />
            ))}
            {hover !== null && (
              <g pointerEvents="none">
                <line
                  x1={x(traffic.t[hover])}
                  x2={x(traffic.t[hover])}
                  y1={margin.top}
                  y2={margin.top + plotHeight}
                  stroke="#d4d4d4"
                />
                {[
                  { value: up[hover], color: UP },
                  { value: down[hover], color: DOWN }
                ].map(
                  ({ value, color }) =>
                    value !== null &&
                    value !== undefined && (
                      <circle
                        key={color}
                        cx={x(traffic.t[hover])}
                        cy={y(value)}
                        r={4}
                        fill={color}
                        stroke="white"
                        strokeWidth={2}
                      />
                    )
                )}
              </g>
            )}
            <rect
              x={margin.left}
              y={margin.top}
              width={plotWidth}
              height={plotHeight}
              fill="transparent"
              onPointerMove={onMove}
              onPointerLeave={() => setHover(null)}
            />
          </svg>
        )}
        {hover !== null && (
          <div
            className="pointer-events-none absolute top-2 rounded-md border border-line bg-white px-3 py-2 text-xs text-body shadow-pop"
            style={
              x(traffic.t[hover]) > width / 2
                ? { right: width - x(traffic.t[hover]) + 12 }
                : { left: x(traffic.t[hover]) + 12 }
            }
          >
            <p className="mb-1 text-muted">{formatTime(traffic.t[hover], 0)}</p>
            <p>上り {up[hover] === null ? '—' : `${formatMbps(up[hover] ?? 0)} Mbps`}</p>
            <p>下り {down[hover] === null ? '—' : `${formatMbps(down[hover] ?? 0)} Mbps`}</p>
            <p>セッション {traffic.series.sessions[hover] ?? 0}</p>
          </div>
        )}
      </div>
    </div>
  )
}

function Bar({ value, max, color, label }: { value: number; max: number; color: string; label: string }) {
  const share = max > 0 ? value / max : 0
  return (
    <div className="flex items-center gap-2" title={`${label} ${formatBytes(value)}`}>
      <div className="h-3 flex-1">
        <div
          className="h-full rounded-r-[4px]"
          style={{ width: `${Math.max(share * 100, value > 0 ? 0.5 : 0)}%`, background: color }}
        />
      </div>
      <span className="w-16 shrink-0 text-right text-xs text-body tabular-nums">{formatBytes(value)}</span>
    </div>
  )
}

function RelayBars({ relays }: { relays: Usage['relays'] }) {
  if (relays.length === 0) {
    return <Empty text="この期間の通信はありません。" />
  }
  const max = Math.max(...relays.flatMap((relay) => [relay.sent_bytes, relay.received_bytes]))
  return (
    <div className="flex flex-col gap-4">
      <Legend />
      {relays.map((relay) => (
        <div key={relay.relay_id} className="flex flex-col gap-0.5">
          <p className="text-[13px] text-strong">
            {relay.relay_id} <span className="text-xs text-muted">· {relay.sessions} セッション</span>
          </p>
          <Bar value={relay.received_bytes} max={max} color={UP} label="上り" />
          <Bar value={relay.sent_bytes} max={max} color={DOWN} label="下り" />
        </div>
      ))}
    </div>
  )
}

function bucketLabel(index: number): string {
  if (index === 0) return `〜${formatBytes(SESSION_BUCKETS[0])}`
  if (index === SESSION_BUCKETS.length) return `${formatBytes(SESSION_BUCKETS[index - 1])}〜`
  return `${formatBytes(SESSION_BUCKETS[index - 1])}〜${formatBytes(SESSION_BUCKETS[index])}`
}

/** How many sessions moved how much, up and down together, on a decade scale. */
function SessionHistogram({ sessions }: { sessions: Usage['sessions'] }) {
  if (sessions.length === 0) {
    return <Empty text="この期間のセッションはありません。" />
  }
  const counts = Array.from({ length: SESSION_BUCKETS.length + 1 }, () => 0)
  for (const session of sessions) {
    const bytes = session.sent_bytes + session.received_bytes
    const bucket = SESSION_BUCKETS.findIndex((bound) => bytes < bound)
    counts[bucket === -1 ? SESSION_BUCKETS.length : bucket] += 1
  }
  const max = Math.max(...counts)
  return (
    <div className="flex flex-col gap-2">
      <div className="flex h-40 items-end gap-0.5 border-b border-line">
        {counts.map((count, index) => (
          <div
            key={index}
            className="flex h-full flex-1 flex-col items-center justify-end gap-1"
            title={`${bucketLabel(index)}: ${count} セッション`}
          >
            <span className="text-xs text-body tabular-nums">{count}</span>
            <div
              className="w-full max-w-6 rounded-t-[4px]"
              style={{ height: `${max > 0 ? (count / max) * 85 : 0}%`, background: UP }}
            />
          </div>
        ))}
      </div>
      <div className="flex gap-0.5">
        {counts.map((_, index) => (
          <span key={index} className="flex-1 text-center text-[11px] text-muted">
            {bucketLabel(index)}
          </span>
        ))}
      </div>
      <p className="text-xs text-subtle">
        1 セッションの上りと下りの合計。通信量の多い {sessions.length.toLocaleString()} セッションまでを数えます。
      </p>
    </div>
  )
}

function NamespaceTable({ namespaces }: { namespaces: Usage['namespaces'] }) {
  if (namespaces.length === 0) {
    return <Empty text="この期間の通信はありません。" />
  }
  const max = Math.max(...namespaces.flatMap((row) => [row.sent_bytes, row.received_bytes]))
  return (
    <table className="w-full text-left text-[13px]">
      <thead>
        <tr className="border-b border-line text-xs text-muted">
          <th className="py-2 font-normal">Track Namespace</th>
          <th className="w-[30%] py-2 font-normal">上り（配信者 → relay）</th>
          <th className="w-[30%] py-2 font-normal">下り（relay → 視聴者）</th>
        </tr>
      </thead>
      <tbody>
        {namespaces.map((row) => (
          <tr key={row.namespace} className="border-b border-divider last:border-0">
            <td className="py-2 pr-4 font-mono text-xs break-all text-strong">
              {row.namespace.split('/').slice(1).join('/') || '(app のルート)'}
            </td>
            <td className="py-2 pr-4">
              <Bar value={row.received_bytes} max={max} color={UP} label="上り" />
            </td>
            <td className="py-2">
              <Bar value={row.sent_bytes} max={max} color={DOWN} label="下り" />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
