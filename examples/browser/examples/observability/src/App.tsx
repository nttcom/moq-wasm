import { useEffect, useMemo, useRef, useState } from 'react'
import { type RelaySnapshot, fetchSnapshots } from './api'
import type { NamespaceSummary } from './api'
import { ChartDrawer } from './ChartDrawer'
import { NAMESPACE_PANEL_WIDTH, NamespacePanel } from './NamespacePanel'
import { layoutTopology } from './layout'
import { type Filters, type Selection, narrowedFilters, routesOf, visibility, widenedFilters } from './selection'
import { RANGE_NAMES, RANGES, type RangeName } from './timeRange'
import { TopologyView } from './TopologyView'
import { appIdOf, buildTopology, relativeNamespace } from './topology'

const POLL_MS = 1_000
const PAST_RATE_WINDOW_MS = 2_000
const SEEK_STEP_MS = 1_000
const COARSE_STEP_MS = 10_000
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
  return new Date(atMs).toLocaleString([], {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit'
  })
}

const handlesArrowKeys = (target: EventTarget | null) =>
  target instanceof HTMLElement && ['INPUT', 'SELECT', 'TEXTAREA'].includes(target.tagName)

export default function App() {
  const [snapshots, setSnapshots] = useState<RelaySnapshot[]>([])
  const [previous, setPrevious] = useState<Map<string, RelaySnapshot>>(new Map())
  const [error, setError] = useState<string | null>(null)
  const [filters, setFilters] = useState<Filters>({ appId: '', namespacePrefix: '' })
  const [chosenAppId, setChosenAppId] = useState('')
  const [selection, setSelection] = useState<Selection>(null)
  const [drawerClosed, setDrawerClosed] = useState(false)
  const [namespacesOpen, setNamespacesOpen] = useState(false)
  const [drawerHeight, setDrawerHeight] = useState(() => Math.round(window.innerHeight * DEFAULT_DRAWER_SHARE))
  const [range, setRange] = useState<RangeName>('1h')
  const [atMs, setAtMs] = useState<number | null>(null)
  const [nowMs, setNowMs] = useState(Date.now())
  const latestRef = useRef<RelaySnapshot[]>([])

  const live = atMs === null
  const windowStartMs = nowMs - RANGES[range]
  const seekTo = (ms: number) =>
    setAtMs(ms >= nowMs - SEEK_STEP_MS ? null : Math.round(Math.max(nowMs - RANGES[range], ms)))
  const stepBy = (deltaMs: number) => seekTo((atMs ?? nowMs) + deltaMs)
  const changeRange = (next: RangeName) => {
    setRange(next)
    if (atMs !== null) setAtMs(Math.max(nowMs - RANGES[next], atMs))
  }

  const stepByRef = useRef(stepBy)
  stepByRef.current = stepBy
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (handlesArrowKeys(event.target) || (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight')) return
      event.preventDefault()
      const step = event.shiftKey ? COARSE_STEP_MS : SEEK_STEP_MS
      stepByRef.current(event.key === 'ArrowLeft' ? -step : step)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

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
    if (atMs === null) return
    let stopped = false
    const target = atMs
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
  }, [atMs])

  const topology = useMemo(() => buildTopology(snapshots, previous), [snapshots, previous])
  const layout = useMemo(() => layoutTopology(topology), [topology])
  const visible = useMemo(() => visibility(topology, filters), [topology, filters])

  useEffect(() => {
    if (!selection) return
    const exists = {
      mesh: () => true,
      relay: () => topology.relays.some((relay) => relay.id === selection.id),
      client: () => visible.clients.has(selection.id),
      link: () => visible.links.has(selection.id)
    }[selection.kind]()
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

  const focusOn = (next: Selection) => {
    if (next?.kind !== 'mesh') setFilters(narrowedFilters(routesOf(topology, next, visible), filters))
    setSelection(next)
    setDrawerClosed(false)
  }

  const openNamespace = (summary: NamespaceSummary, stillPublished: boolean) => {
    setFilters({ appId: appIdOf(summary.namespace), namespacePrefix: relativeNamespace(summary.namespace) })
    setSelection(null)
    if (stillPublished) setAtMs(null)
    else seekTo(summary.last_seen_ms)
    setNamespacesOpen(false)
  }

  const onBackground = () => {
    setFilters(widenedFilters(filters, chosenAppId))
    setSelection(null)
  }

  const drawerOpen = Boolean(selection) && !drawerClosed
  const selectionTitle = (() => {
    if (!selection) return ''
    if (selection.kind === 'mesh') return 'All relays'
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
        <button
          className={`panel-toggle${namespacesOpen ? ' on' : ''}`}
          onClick={() => setNamespacesOpen(!namespacesOpen)}
        >
          Namespaces
        </button>
        <div className="timeline" title="← → move 1 s, Shift + ← → move 10 s">
          <div className="ranges">
            {RANGE_NAMES.map((name) => (
              <button key={name} className={name === range ? 'on' : ''} onClick={() => changeRange(name)}>
                {name}
              </button>
            ))}
          </div>
          <button className="step" aria-label="Back 10 seconds" onClick={() => stepBy(-COARSE_STEP_MS)}>
            −10s
          </button>
          <input
            type="range"
            min={0}
            max={RANGES[range] / SEEK_STEP_MS}
            step={1}
            value={((atMs ?? nowMs) - windowStartMs) / SEEK_STEP_MS}
            aria-label="Snapshot time"
            onChange={(event) => seekTo(windowStartMs + Number(event.target.value) * SEEK_STEP_MS)}
          />
          <button className="step" aria-label="Forward 10 seconds" onClick={() => stepBy(COARSE_STEP_MS)}>
            +10s
          </button>
          <button className={`live${live ? '' : ' past'}`} onClick={() => setAtMs(null)}>
            <i />
            {live ? 'Live' : seekLabel(atMs)}
          </button>
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
          leftInset={namespacesOpen ? NAMESPACE_PANEL_WIDTH : 0}
          onSelect={focusOn}
          onBackground={onBackground}
        />
        <div
          className="legend"
          style={{
            bottom: (drawerOpen ? drawerHeight : 0) + 12,
            left: (namespacesOpen ? NAMESPACE_PANEL_WIDTH : 0) + 16
          }}
        >
          <span className="focus">shown in charts</span>
          <span className="up">uplink</span>
          <span className="ok">downlink loss &lt; 1%</span>
          <span className="warn">downlink loss 1–5%</span>
          <span className="bad">downlink loss &gt; 5%</span>
        </div>
        {namespacesOpen && (
          <NamespacePanel
            spanMs={RANGES[range]}
            toMs={nowMs}
            live={live}
            appId={filters.appId}
            onPick={openNamespace}
            onClose={() => setNamespacesOpen(false)}
          />
        )}
        {drawerOpen && (
          <ChartDrawer
            topology={topology}
            selection={selection}
            visible={visible}
            title={selectionTitle}
            range={range}
            onRangeChange={changeRange}
            windowEndMs={nowMs}
            markMs={atMs}
            onSeek={seekTo}
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
