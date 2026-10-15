import { type ReactNode, useEffect, useMemo, useState } from 'react'
import { type RelaySnapshot, type Series, fetchSeries } from './api'
import { type ChartGroup, type ChartSpec, chartGroups } from './charts'
import { type ChartLine, LineChart, formatValue } from './LineChart'
import { type Selection, type Visibility, routesOf } from './selection'
import { RANGE_NAMES, RANGES, type RangeName } from './timeRange'
import { type Topology, drainingSeconds, isStatsSession, linkMbps, relativeNamespace } from './topology'

const POINTS = 90
const LIVE_REFRESH_MS = 5_000
const MIN_HEIGHT = 160
const PALETTE = ['#85B7EB', '#5DCAA5', '#EF9F27', '#ED93B1', '#AFA9EC']
const TOTAL_COLOR = '#e8e8e6'

function withTotal(lines: ChartLine[], totalLabel: string | undefined): ChartLine[] {
  if (!totalLabel || lines.length < 2) return lines
  const sums = new Map<number, number>()
  for (const line of lines) {
    line.t.forEach((ms, index) => {
      const value = line.v[index]
      if (value !== null) sums.set(ms, (sums.get(ms) ?? 0) + value)
    })
  }
  const t = [...sums.keys()].sort((a, b) => a - b)
  return [...lines, { label: totalLabel, color: TOTAL_COLOR, t, v: t.map((ms) => sums.get(ms) ?? null) }]
}

interface Props {
  topology: Topology
  selection: Selection
  visible: Visibility
  title: string
  range: RangeName
  onRangeChange: (range: RangeName) => void
  windowEndMs: number
  markMs: number | null
  onSeek: (ms: number) => void
  height: number
  onResize: (height: number) => void
  onClose: () => void
}

const requestKey = (relayId: string, target: object) => `${relayId}|${JSON.stringify(target)}`

function Section({ title, direction, children }: { title: string; direction?: string; children: ReactNode }) {
  return (
    <>
      <div className="section">
        <span>{title}</span>
        {direction && <small>{direction}</small>}
      </div>
      {children}
    </>
  )
}

function Rows({ rows }: { rows: [ReactNode, ReactNode?][] }) {
  return (
    <div className="rows">
      {rows.map(([left, right], index) => (
        <div key={index}>
          <span>{left}</span>
          <span>{right}</span>
        </div>
      ))}
    </div>
  )
}

const mbps = (value: number) => `${formatValue(value)} Mbps`
const relayState = (snapshot: RelaySnapshot) => {
  const draining = drainingSeconds(snapshot)
  return draining === null ? 'serving' : `draining ${draining}s, ${snapshot.process.client_sessions} clients left`
}
const lagText = (us: number) => `lag ${Math.round(us / 1000)} ms`

function ScopeNote({ visible, namespacePrefix }: { visible: Visibility; namespacePrefix: string }) {
  if (!visible.trackKeys || !namespacePrefix) return null
  return (
    <div className="note">
      Bitrate and tracks: <b>{namespacePrefix}</b> only. RTT, loss, cwnd and the other transport figures cover the whole
      session.
    </div>
  )
}

const nodeLabel = (topology: Topology, id: string) => topology.clients.get(id)?.label ?? id

function Tabs({ groups, active, onPick }: { groups: ChartGroup[]; active: string; onPick: (name: string) => void }) {
  if (groups.length < 2) return null
  return (
    <div className="tabs" role="tablist">
      {groups.map((group) => (
        <button
          key={group.name}
          role="tab"
          aria-selected={group.name === active}
          className={group.name === active ? 'on' : ''}
          onClick={() => onPick(group.name)}
        >
          {group.name}
        </button>
      ))}
    </div>
  )
}

function Details({
  topology,
  selection,
  visible,
  namespacePrefix,
  tabs
}: Pick<Props, 'topology' | 'selection' | 'visible'> & { namespacePrefix: string; tabs: ReactNode }) {
  if (!selection) return null
  const routes = routesOf(topology, selection, visible)
  const routeRows = routes.map((route): [ReactNode, ReactNode] => [
    [route.hops[0].split('>')[0], ...route.hops.map((hop) => hop.split('>')[1])]
      .map((id) => nodeLabel(topology, id))
      .join(' → '),
    relativeNamespace(route.namespace)
  ])
  if (selection.kind === 'mesh') {
    const links = [...topology.links.values()].filter((link) => visible.links.has(link.key))
    const throughput = (relayId: string, direction: 'from' | 'to') =>
      links
        .filter((link) => link[direction] === relayId)
        .reduce((sum, link) => sum + linkMbps(link, visible.trackKeys), 0)
    return (
      <>
        <div className="sub">
          {topology.relays.length} relays · {visible.clients.size} clients · {visible.routes.length} subscriptions
        </div>
        <ScopeNote visible={visible} namespacePrefix={namespacePrefix} />
        {tabs}
        {topology.relays.map((relay) => (
          <Section key={relay.id} title={relay.id}>
            <Rows
              rows={[
                [
                  'Clients',
                  [...topology.clients.values()].filter(
                    (client) => client.relayId === relay.id && visible.clients.has(client.key)
                  ).length
                ],
                ['State', relayState(relay.snapshot)],
                ['Ingress', mbps(throughput(relay.id, 'to'))],
                ['Egress', mbps(throughput(relay.id, 'from'))],
                [
                  'Memory',
                  relay.snapshot.process.rss_bytes === null
                    ? '—'
                    : `${Math.round(relay.snapshot.process.rss_bytes / 1024 / 1024)} MB`
                ]
              ]}
            />
          </Section>
        ))}
      </>
    )
  }
  if (selection.kind === 'relay') {
    const relay = topology.relays.find((candidate) => candidate.id === selection.id)
    if (!relay) return null
    return (
      <>
        <div className="sub">
          {relay.snapshot.sessions.filter((session) => !isStatsSession(session)).length} sessions ·{' '}
          {relayState(relay.snapshot)}
        </div>
        <ScopeNote visible={visible} namespacePrefix={namespacePrefix} />
        {tabs}
      </>
    )
  }
  if (selection.kind === 'client') {
    const client = topology.clients.get(selection.id)
    if (!client) return null
    const relay = topology.relays.find((candidate) => candidate.id === client.relayId)?.snapshot
    const published = (relay?.tracks ?? []).filter((track) => track.publisher_session_id === client.session.session_id)
    const received = routes.filter((route) => route.subscriber === client.key)
    const subscribers = new Set(
      routes.filter((route) => route.publisher === client.key).map((route) => route.subscriber)
    ).size
    return (
      <>
        <div className="sub">
          {client.label} via {client.relayId} · MTU {client.session.current_mtu} B
          <br />
          app {client.appId}
        </div>
        <ScopeNote visible={visible} namespacePrefix={namespacePrefix} />
        {tabs}
        {published.length > 0 && (
          <Section title="↑ Publishes" direction={`${client.label} → ${client.relayId}`}>
            <Rows
              rows={[
                ...published.map((track): [ReactNode] => [`${relativeNamespace(track.namespace)} / ${track.name}`]),
                ['Subscribers', subscribers]
              ]}
            />
          </Section>
        )}
        {received.length > 0 && (
          <Section title="↓ Receives" direction={`${client.relayId} → ${client.label}`}>
            <Rows
              rows={received.map((route): [ReactNode, ReactNode] => [
                `${relativeNamespace(route.namespace)} / ${route.name}`,
                lagText(route.subscription.lag_behind_newest_received_us)
              ])}
            />
          </Section>
        )}
      </>
    )
  }
  const link = topology.links.get(selection.id)
  if (!link) return null
  const measuredBy = link.measured
    ? `measured by ${link.measured.relayId} (${link.measured.bySender ? 'sender' : 'receiver'})`
    : 'no session statistics'
  const tracks = [...new Set(routes.map((route) => `${relativeNamespace(route.namespace)} / ${route.name}`))]
  return (
    <>
      <div className="sub">
        {link.kind} · {measuredBy}
      </div>
      <ScopeNote visible={visible} namespacePrefix={namespacePrefix} />
      {tabs}
      <Section title="Tracks" direction={`${nodeLabel(topology, link.from)} → ${nodeLabel(topology, link.to)}`}>
        <Rows rows={tracks.map((track): [ReactNode] => [track])} />
      </Section>
      <Section title="Routes: original publisher → subscriber" direction={String(routeRows.length)}>
        <Rows rows={routeRows} />
      </Section>
    </>
  )
}

export function ChartDrawer(props: Props & { namespacePrefix: string }) {
  const { topology, selection, visible, title, range, onRangeChange, windowEndMs, markMs, onSeek } = props
  const { height, onResize, onClose, namespacePrefix } = props
  const [loaded, setLoaded] = useState<Map<string, Series>>(new Map())
  const [hoverMs, setHoverMs] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)

  const [groupName, setGroupName] = useState<string | null>(null)
  const groups = useMemo(() => chartGroups(topology, selection, visible), [topology, selection, visible])
  const group = groups.find((candidate) => candidate.name === groupName) ?? groups[0]
  const specs: ChartSpec[] = useMemo(() => group?.charts ?? [], [group])
  const requests = useMemo(() => {
    const unique = new Map<string, { relayId: string; target: ChartSpec['lines'][number]['target'] }>()
    for (const spec of specs) {
      for (const line of spec.lines) unique.set(requestKey(line.relayId, line.target), line)
    }
    return unique
  }, [specs])
  const requestSignature = [...requests.keys()].sort().join(';')
  const fetchEndMs = Math.floor(windowEndMs / LIVE_REFRESH_MS) * LIVE_REFRESH_MS
  const fromMs = fetchEndMs - RANGES[range]

  useEffect(() => {
    let cancelled = false
    Promise.all(
      [...requests].map(
        async ([key, { relayId, target }]) =>
          [key, await fetchSeries(relayId, target, fromMs, fetchEndMs, POINTS)] as const
      )
    )
      .then((entries) => {
        if (cancelled) return
        setLoaded(new Map(entries))
        setError(null)
      })
      .catch((reason: unknown) => {
        if (!cancelled) setError(String(reason))
      })
    return () => {
      cancelled = true
    }
  }, [requestSignature, fromMs, fetchEndMs])

  const onGripDown = (event: React.PointerEvent<HTMLDivElement>) => {
    const grip = event.currentTarget
    const main = grip.closest('main')
    if (!main) return
    const bottom = main.getBoundingClientRect().bottom
    const maxHeight = main.clientHeight - 60
    grip.setPointerCapture(event.pointerId)
    const move = (moveEvent: PointerEvent) =>
      onResize(Math.round(Math.max(MIN_HEIGHT, Math.min(maxHeight, bottom - moveEvent.clientY))))
    const up = () => {
      grip.removeEventListener('pointermove', move)
      grip.removeEventListener('pointerup', up)
    }
    grip.addEventListener('pointermove', move)
    grip.addEventListener('pointerup', up)
  }

  return (
    <section className="drawer" style={{ height }} aria-label="Charts">
      <div className="drawer-grip" title="Drag to resize" onPointerDown={onGripDown} />
      <div className="drawer-head">
        <b>{title}</b>
        <small>{hoverMs === null ? 'latest' : new Date(hoverMs).toLocaleString()}</small>
        {error && <small className="error">{error}</small>}
        <div className="ranges">
          {RANGE_NAMES.map((name) => (
            <button key={name} className={name === range ? 'on' : ''} onClick={() => onRangeChange(name)}>
              {name}
            </button>
          ))}
        </div>
        <button className="close" aria-label="Close charts" onClick={onClose}>
          ×
        </button>
      </div>
      <div className="drawer-body">
        <div className="details">
          <Details
            topology={topology}
            selection={selection}
            visible={visible}
            namespacePrefix={namespacePrefix}
            tabs={group && <Tabs groups={groups} active={group.name} onPick={setGroupName} />}
          />
        </div>
        <div className="charts">
          {specs.map((spec) => (
            <LineChart
              key={spec.title}
              title={spec.title}
              unit={spec.unit}
              fromMs={fromMs}
              toMs={fetchEndMs}
              hoverMs={hoverMs}
              onHover={setHoverMs}
              markMs={markMs}
              onPick={onSeek}
              lines={withTotal(
                spec.lines.map((line, index) => {
                  const series = loaded.get(requestKey(line.relayId, line.target))
                  // Servers older than the metric omit it.
                  const values = series?.series[line.metric]
                  return {
                    label: line.label,
                    color: PALETTE[index % PALETTE.length],
                    t: values ? series.t : [],
                    v: values ?? []
                  }
                }),
                spec.totalLabel
              )}
            />
          ))}
        </div>
      </div>
    </section>
  )
}
