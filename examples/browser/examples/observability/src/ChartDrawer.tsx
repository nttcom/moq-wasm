import { useEffect, useMemo, useState } from 'react'
import { type Series, fetchSeries } from './api'
import { type ChartGroup, type ChartSpec, chartGroups } from './charts'
import { type ChartLine, LineChart } from './LineChart'
import { type Selection, type Visibility } from './selection'
import { RANGE_NAMES, RANGES, type RangeName } from './timeRange'
import { type Topology } from './topology'

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

function Tabs({ groups, active, onPick }: { groups: ChartGroup[]; active: string; onPick: (name: string) => void }) {
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

export function ChartDrawer(props: Props) {
  const { topology, selection, visible, title, range, onRangeChange, windowEndMs, markMs, onSeek } = props
  const { height, onResize, onClose } = props
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
        {group && groups.length > 1 && (
          <div className="details">
            <Tabs groups={groups} active={group.name} onPick={setGroupName} />
          </div>
        )}
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
