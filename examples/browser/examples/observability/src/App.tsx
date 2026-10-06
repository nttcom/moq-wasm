import { useEffect, useMemo, useRef, useState } from 'react'
import { type RelaySnapshot, fetchSnapshots } from './api'
import { ChartDrawer } from './ChartDrawer'
import { layoutTopology } from './layout'
import { formatValue } from './LineChart'
import { type Filters, type Selection, narrowedFilters, routesOf, visibility, widenedFilters } from './selection'
import { TopologyView } from './TopologyView'
import { buildTopology, linkMbps, relativeNamespace } from './topology'

const POLL_MS = 1_000
const PAST_RATE_WINDOW_MS = 2_000
const SEEK_SPAN_MS = 7 * 86_400_000
const SEEK_STEP_MS = 1_000
const SEEK_STEPS = SEEK_SPAN_MS / SEEK_STEP_MS
const DEFAULT_DRAWER_SHARE = 0.45

const byRelay = (snapshots: RelaySnapshot[]) => new Map(snapshots.map((snapshot) => [snapshot.relay_id, snapshot]))

function advancedPrevious(
  previous: Map<string, RelaySnapshot>,
  latest: RelaySnapshot[],
  next: RelaySnapshot[]
): Map<string, RelaySnapshot> {
  const latestByRelay = byRelay(latest)
  const advanced = new Map(previous)
  for (const snapshot of next) {
    const last = latestByRelay.get(snapshot.relay_id)
    if (last && last.timestamp_ms < snapshot.timestamp_ms) advanced.set(snapshot.relay_id, last)
  }
  return advanced
}

function seekLabel(atMs: number): string {
  return new Date(atMs).toLocaleString([], { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
}

export default function App() {
  const [snapshots, setSnapshots] = useState<RelaySnapshot[]>([])
  const [previous, setPrevious] = useState<Map<string, RelaySnapshot>>(new Map())
  const [error, setError] = useState<string | null>(null)
  const [filters, setFilters] = useState<Filters>({ appId: '', namespacePrefix: '' })
  const [chosenAppId, setChosenAppId] = useState('')
  const [selection, setSelection] = useState<Selection>(null)
  const [drawerClosed, setDrawerClosed] = useState(false)
  const [drawerHeight, setDrawerHeight] = useState(() => Math.round(window.innerHeight * DEFAULT_DRAWER_SHARE))
  const [seekStep, setSeekStep] = useState(SEEK_STEPS)
  const [nowMs, setNowMs] = useState(Date.now())
  const latestRef = useRef<RelaySnapshot[]>([])

  const live = seekStep === SEEK_STEPS
  const atMs = nowMs - (SEEK_STEPS - seekStep) * SEEK_STEP_MS

  useEffect(() => {
    if (!live) return
    let stopped = false
    const poll = async () => {
      try {
        const next = await fetchSnapshots()
        if (stopped) return
        setPrevious((previous) => advancedPrevious(previous, latestRef.current, next))
        latestRef.current = next
        setSnapshots(next)
        setError(null)
      } catch (reason) {
        if (!stopped) setError(String(reason))
      }
      if (!stopped) setNowMs(Date.now())
    }
    poll()
    const timer = setInterval(poll, POLL_MS)
    return () => {
      stopped = true
      clearInterval(timer)
    }
  }, [live])

  useEffect(() => {
    if (live) return
    let stopped = false
    const target = Math.round(atMs)
    Promise.all([fetchSnapshots(target), fetchSnapshots(target - PAST_RATE_WINDOW_MS)])
      .then(([current, before]) => {
        if (stopped) return
        setPrevious(byRelay(before))
        latestRef.current = current
        setSnapshots(current)
        setError(null)
      })
      .catch((reason: unknown) => {
        if (!stopped) setError(String(reason))
      })
    return () => {
      stopped = true
    }
  }, [live, atMs])

  const topology = useMemo(() => buildTopology(snapshots, previous), [snapshots, previous])
  const layout = useMemo(() => layoutTopology(topology), [topology])
  const visible = useMemo(() => visibility(topology, filters), [topology, filters])

  useEffect(() => {
    if (!selection) return
    const exists =
      selection.kind === 'relay'
        ? topology.relays.some((relay) => relay.id === selection.id)
        : selection.kind === 'client'
          ? visible.clients.has(selection.id)
          : visible.links.has(selection.id)
    if (!exists) setSelection(null)
  }, [topology, visible, selection])

  const appIds = useMemo(
    () => [...new Set([...topology.clients.values()].map((client) => client.appId))].sort(),
    [topology]
  )
  const namespaceOptions = useMemo(() => {
    const prefixes = new Set<string>()
    for (const route of topology.routes) {
      if (filters.appId && route.appId !== filters.appId) continue
      const elements = relativeNamespace(route.namespace).split('/')
      elements.forEach((_, index) => prefixes.add(elements.slice(0, index + 1).join('/')))
    }
    return [...prefixes].sort()
  }, [topology, filters.appId])

  const egressMbps = [...topology.links.values()]
    .filter((link) => link.kind !== 'uplink' && visible.links.has(link.key))
    .reduce((sum, link) => sum + linkMbps(link, visible.trackKeys), 0)

  const focusOn = (next: Selection) => {
    const narrowed = narrowedFilters(routesOf(topology, next, visible), filters)
    setFilters(narrowed)
    setSelection(next)
    setDrawerClosed(false)
  }

  const onBackground = () => {
    setFilters(widenedFilters(filters, chosenAppId))
    setSelection(null)
  }

  const drawerOpen = Boolean(selection) && !drawerClosed
  const selectionTitle = (() => {
    if (!selection) return ''
    if (selection.kind === 'client') return topology.clients.get(selection.id)?.label ?? selection.id
    if (selection.kind === 'link') {
      const [from, to] = selection.id.split('>')
      const label = (id: string) => topology.clients.get(id)?.label ?? id
      return `${label(from)} → ${label(to)}`
    }
    return selection.id
  })()

  return (
    <div className="app">
      <header>
        <h1>MoQ Observability</h1>
        <label className="filter">
          App
          <select
            value={filters.appId}
            onChange={(event) => {
              setChosenAppId(event.target.value)
              setFilters({ appId: event.target.value, namespacePrefix: '' })
            }}
          >
            <option value="">All apps</option>
            {appIds.map((appId) => (
              <option key={appId} value={appId}>
                {appId.length > 12 ? `${appId.slice(0, 8)}…` : appId}
              </option>
            ))}
          </select>
        </label>
        <label className="filter">
          Track Namespace
          <input
            className={filters.namespacePrefix ? 'active' : ''}
            list="namespace-options"
            placeholder="prefix, e.g. room1"
            autoComplete="off"
            value={filters.namespacePrefix}
            onChange={(event) => setFilters({ ...filters, namespacePrefix: event.target.value })}
          />
          <datalist id="namespace-options">
            {namespaceOptions.map((prefix) => (
              <option key={prefix} value={prefix} />
            ))}
          </datalist>
          <button aria-label="Clear namespace filter" onClick={() => setFilters({ ...filters, namespacePrefix: '' })}>
            ×
          </button>
        </label>
        <div className="timeline" title="Seek back up to 7 days">
          <span>7d ago</span>
          <input
            type="range"
            min={0}
            max={SEEK_STEPS}
            step={1}
            value={seekStep}
            aria-label="Snapshot time"
            onChange={(event) => setSeekStep(Number(event.target.value))}
          />
          <button className={`live${live ? '' : ' past'}`} onClick={() => setSeekStep(SEEK_STEPS)}>
            <i />
            {live ? 'Live' : seekLabel(atMs)}
          </button>
        </div>
        <div className="stats">
          <span>
            Relays <b>{topology.relays.length}</b>
          </span>
          <span>
            Clients <b>{visible.clients.size}</b>
          </span>
          <span>
            Subscriptions <b>{visible.routes.length}</b>
          </span>
          <span>
            Egress <b>{formatValue(egressMbps)} Mbps</b>
          </span>
        </div>
      </header>
      {error && <div className="banner">{error}</div>}
      <main>
        <TopologyView
          topology={topology}
          layout={layout}
          visible={visible}
          selection={selection}
          bottomInset={drawerOpen ? drawerHeight : 0}
          onSelect={focusOn}
          onBackground={onBackground}
        />
        <div className="legend" style={{ bottom: (drawerOpen ? drawerHeight : 0) + 12 }}>
          <span className="focus">shown in charts</span>
          <span className="up">uplink</span>
          <span className="ok">downlink loss &lt; 1%</span>
          <span className="warn">downlink loss 1–5%</span>
          <span className="bad">downlink loss &gt; 5%</span>
        </div>
        {drawerOpen && (
          <ChartDrawer
            topology={topology}
            selection={selection}
            visible={visible}
            title={selectionTitle}
            endMs={live ? nowMs : atMs}
            height={drawerHeight}
            onResize={setDrawerHeight}
            onClose={() => setDrawerClosed(true)}
            namespacePrefix={filters.namespacePrefix}
          />
        )}
      </main>
    </div>
  )
}
