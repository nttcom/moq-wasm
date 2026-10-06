import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { CENTER, CLIENT_HALF, ENVIRONMENT_RADIUS, type HalfSize, type Layout, type Point, RELAY_HALF } from './layout'
import { MESH, type Selection, type Visibility, routesOf } from './selection'
import { type Link, type Topology, linkMbps } from './topology'

interface ViewBox {
  x: number
  y: number
  w: number
  h: number
}

interface Props {
  topology: Topology
  layout: Layout
  visible: Visibility
  selection: Selection
  bottomInset: number
  leftInset: number
  onSelect: (selection: Selection) => void
  onBackground: () => void
}

const FIT_PADDING = 40
const OVERVIEW_PADDING = 80
const ENVIRONMENT_LABEL_HEIGHT = 12
const MAX_ZOOM_IN = 1 / 0.6
const MIN_VIEW_WIDTH = 150
const MAX_VIEW_WIDTH = 8000
const PAN_THRESHOLD_PX = 4
const ANIMATION_MS = 350
const LINK_OFFSET = 4
const ZOOM_STEP = 1.25

const halfOf = (topology: Topology, id: string): HalfSize => (topology.clients.has(id) ? CLIENT_HALF : RELAY_HALF)

function edgeDistance(half: HalfSize, dx: number, dy: number, length: number): number {
  return Math.min(half.w / Math.abs(dx / length || 1e-9), half.h / Math.abs(dy / length || 1e-9))
}

function linkPath(topology: Topology, layout: Layout, link: Link): string | null {
  const a = layout.clients.get(link.from) ?? layout.relays.get(link.from)
  const b = layout.clients.get(link.to) ?? layout.relays.get(link.to)
  if (!a || !b) return null
  const dx = b.x - a.x
  const dy = b.y - a.y
  const length = Math.hypot(dx, dy) || 1
  const nx = (-dy / length) * LINK_OFFSET
  const ny = (dx / length) * LINK_OFFSET
  const start = edgeDistance(halfOf(topology, link.from), dx, dy, length) + 2
  const end = edgeDistance(halfOf(topology, link.to), dx, dy, length) + 6
  return `M${a.x + (dx / length) * start + nx},${a.y + (dy / length) * start + ny} L${b.x - (dx / length) * end + nx},${b.y - (dy / length) * end + ny}`
}

const WARN_LOSS_PERCENT = 1
const BAD_LOSS_PERCENT = 5

function linkColorClass(link: Link): string {
  if (link.kind === 'uplink' || link.lossPercent === null) return 'up'
  if (link.lossPercent < WARN_LOSS_PERCENT) return 'ok'
  return link.lossPercent <= BAD_LOSS_PERCENT ? 'warn' : 'bad'
}

const formatMegabytes = (bytes: number | null) => (bytes === null ? '—' : `${Math.round(bytes / 1024 / 1024)} MB`)

const MAX_LABEL_CHARS = 16
const MAX_LABEL_CHARS_BESIDE_BADGE = 12

const truncated = (text: string, max = MAX_LABEL_CHARS) => (text.length > max ? `${text.slice(0, max - 1)}…` : text)

function labelOf(namespaces: string[]): string {
  if (namespaces.length === 0) return ''
  return namespaces.length > 1 ? `${namespaces[0]} +${namespaces.length - 1}` : namespaces[0]
}

export function TopologyView({
  topology,
  layout,
  visible,
  selection,
  bottomInset,
  leftInset,
  onSelect,
  onBackground
}: Props) {
  const svgRef = useRef<SVGSVGElement>(null)
  const [viewBox, setViewBox] = useState<ViewBox>({ x: -500, y: -420, w: 1000, h: 840 })
  const viewBoxRef = useRef(viewBox)
  const animationRef = useRef<number | null>(null)
  const panRef = useRef<{
    startX: number
    startY: number
    x: number
    y: number
    moved: boolean
    pointerId: number
  } | null>(null)
  const suppressClickRef = useRef(false)

  const applyViewBox = useCallback((next: ViewBox) => {
    viewBoxRef.current = next
    setViewBox(next)
  }, [])

  const related = useMemo(() => routesOf(topology, selection, visible), [topology, selection, visible])
  const activeLinks = useMemo(() => new Set(related.flatMap((route) => route.hops)), [related])
  const showsWholeMesh = !selection || selection.kind === 'mesh'
  const focusedLinks = useMemo(() => {
    if (!selection || selection.kind === 'mesh') return new Set<string>()
    if (selection.kind === 'link') return new Set([selection.id])
    return new Set([...visible.links].filter((key) => key.split('>').includes(selection.id)))
  }, [selection, visible])
  const activeNodes = useMemo(() => {
    const nodes = new Set<string>(selection ? [selection.id] : [])
    for (const key of activeLinks) for (const id of key.split('>')) nodes.add(id)
    return nodes
  }, [activeLinks, selection])

  const visibleNodeIds = useCallback(() => {
    const relays = topology.relays.map((relay) => relay.id)
    const clients = [...visible.clients]
    const all = [...relays, ...clients]
    return showsWholeMesh ? all : all.filter((id) => activeNodes.has(id))
  }, [topology, visible, showsWholeMesh, activeNodes])

  const fitTarget = useCallback(
    (ids: string[], includeEnvironment: boolean): ViewBox | null => {
      const svg = svgRef.current
      if (!svg || ids.length === 0) return null
      const points = ids
        .map((id) => ({ point: layout.clients.get(id) ?? layout.relays.get(id), half: halfOf(topology, id) }))
        .filter((entry): entry is { point: Point; half: HalfSize } => Boolean(entry.point))
      if (points.length === 0) return null
      const padding = includeEnvironment ? OVERVIEW_PADDING : FIT_PADDING
      const environment = includeEnvironment
        ? [
            { x: CENTER.x - ENVIRONMENT_RADIUS, y: CENTER.y - ENVIRONMENT_RADIUS - ENVIRONMENT_LABEL_HEIGHT },
            { x: CENTER.x + ENVIRONMENT_RADIUS, y: CENTER.y + ENVIRONMENT_RADIUS }
          ]
        : []
      const xs = [
        ...points.flatMap(({ point, half }) => [point.x - half.w, point.x + half.w]),
        ...environment.map((p) => p.x)
      ]
      const ys = [
        ...points.flatMap(({ point, half }) => [point.y - half.h, point.y + half.h]),
        ...environment.map((p) => p.y)
      ]
      const x0 = Math.min(...xs) - padding
      const x1 = Math.max(...xs) + padding
      const y0 = Math.min(...ys) - padding
      const y1 = Math.max(...ys) + padding
      const width = svg.clientWidth
      const visibleWidth = Math.max(120, width - leftInset)
      const visibleHeight = Math.max(120, svg.clientHeight - bottomInset)
      const overviewScale = Math.max(1000 / width, 840 / svg.clientHeight)
      const scale = Math.max((x1 - x0) / visibleWidth, (y1 - y0) / visibleHeight, overviewScale / MAX_ZOOM_IN)
      const w = width * scale
      const h = svg.clientHeight * scale
      return {
        x: (x0 + x1) / 2 - (leftInset + visibleWidth / 2) * scale,
        y: (y0 + y1) / 2 - (visibleHeight * scale) / 2,
        w,
        h
      }
    },
    [layout, topology, bottomInset, leftInset]
  )

  const animateTo = useCallback(
    (target: ViewBox | null) => {
      if (!target) return
      if (animationRef.current !== null) cancelAnimationFrame(animationRef.current)
      const from = viewBoxRef.current
      const start = performance.now()
      const step = (now: number) => {
        const t = Math.min(1, (now - start) / ANIMATION_MS)
        const eased = 1 - (1 - t) ** 3
        applyViewBox({
          x: from.x + (target.x - from.x) * eased,
          y: from.y + (target.y - from.y) * eased,
          w: from.w + (target.w - from.w) * eased,
          h: from.h + (target.h - from.h) * eased
        })
        animationRef.current = t < 1 ? requestAnimationFrame(step) : null
      }
      animationRef.current = requestAnimationFrame(step)
    },
    [applyViewBox]
  )

  const fit = useCallback(
    () => animateTo(fitTarget(visibleNodeIds(), showsWholeMesh)),
    [animateTo, fitTarget, visibleNodeIds, selection]
  )

  const hasNodes = topology.relays.length > 0
  const selectionKey = selection ? `${selection.kind}:${selection.id}` : ''
  useEffect(() => {
    if (hasNodes) fit()
  }, [selectionKey, hasNodes, bottomInset > 0, leftInset > 0])

  const zoomAt = useCallback(
    (factor: number, clientX: number, clientY: number) => {
      const svg = svgRef.current
      const matrix = svg?.getScreenCTM()
      if (!svg || !matrix) return
      if (animationRef.current !== null) cancelAnimationFrame(animationRef.current)
      const point = new DOMPoint(clientX, clientY).matrixTransform(matrix.inverse())
      const current = viewBoxRef.current
      const w = Math.min(MAX_VIEW_WIDTH, Math.max(MIN_VIEW_WIDTH, current.w * factor))
      const applied = w / current.w
      applyViewBox({
        x: point.x - (point.x - current.x) * applied,
        y: point.y - (point.y - current.y) * applied,
        w,
        h: current.h * applied
      })
    },
    [applyViewBox]
  )

  useEffect(() => {
    const svg = svgRef.current
    if (!svg) return
    const onWheel = (event: WheelEvent) => {
      event.preventDefault()
      zoomAt(Math.exp(event.deltaY * (event.ctrlKey ? 0.01 : 0.0015)), event.clientX, event.clientY)
    }
    svg.addEventListener('wheel', onWheel, { passive: false })
    return () => svg.removeEventListener('wheel', onWheel)
  }, [zoomAt])

  const zoomFromCenter = (factor: number) => {
    const rect = svgRef.current?.getBoundingClientRect()
    if (rect) {
      zoomAt(factor, rect.left + (rect.width + leftInset) / 2, rect.top + (rect.height - bottomInset) / 2)
    }
  }

  const onPointerDown = (event: React.PointerEvent<SVGSVGElement>) => {
    if (event.button !== 0) return
    const current = viewBoxRef.current
    panRef.current = {
      startX: event.clientX,
      startY: event.clientY,
      x: current.x,
      y: current.y,
      moved: false,
      pointerId: event.pointerId
    }
  }

  const onPointerMove = (event: React.PointerEvent<SVGSVGElement>) => {
    const pan = panRef.current
    const svg = svgRef.current
    if (!pan || !svg) return
    const dx = event.clientX - pan.startX
    const dy = event.clientY - pan.startY
    if (!pan.moved && Math.hypot(dx, dy) < PAN_THRESHOLD_PX) return
    if (!pan.moved) {
      pan.moved = true
      svg.setPointerCapture(pan.pointerId)
    }
    const current = viewBoxRef.current
    const scale = Math.max(current.w / svg.clientWidth, current.h / svg.clientHeight)
    applyViewBox({ ...current, x: pan.x - dx * scale, y: pan.y - dy * scale })
  }

  const onPointerUp = () => {
    suppressClickRef.current = Boolean(panRef.current?.moved)
    panRef.current = null
  }

  const select = (next: Selection) => {
    if (suppressClickRef.current) {
      suppressClickRef.current = false
      return
    }
    onSelect(next)
  }

  const onBackgroundClick = (event: React.MouseEvent<SVGSVGElement>) => {
    if (suppressClickRef.current) {
      suppressClickRef.current = false
      return
    }
    const target = event.target as Element
    if (!target.closest('.node, .hit, .mesh-label')) onBackground()
  }

  const dimmed = (active: boolean) => (!showsWholeMesh && !active ? ' dim' : '')

  return (
    <div className="topology">
      <svg
        ref={svgRef}
        className="graph"
        viewBox={`${viewBox.x} ${viewBox.y} ${viewBox.w} ${viewBox.h}`}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onClick={onBackgroundClick}
        onDoubleClick={(event) => {
          if (!(event.target as Element).closest('.node, .hit, .mesh-label')) fit()
        }}
      >
        <defs>
          {['focus', 'up', 'ok', 'warn', 'bad'].map((name) => (
            <marker
              key={name}
              id={`arrow-${name}`}
              viewBox="0 0 10 10"
              refX="9"
              refY="5"
              markerWidth="7"
              markerHeight="7"
              orient="auto"
            >
              <path d="M0,0 L10,5 L0,10 z" className={`arrow-${name}`} />
            </marker>
          ))}
        </defs>
        <circle cx={CENTER.x} cy={CENTER.y} r={ENVIRONMENT_RADIUS} className="environment" />
        <g
          className={`mesh-label${selection?.kind === 'mesh' ? ' sel' : ''}`}
          onClick={(event) => {
            event.stopPropagation()
            select(MESH)
          }}
        >
          <title>Show all relays</title>
          <rect
            x={CENTER.x - 60}
            y={CENTER.y - ENVIRONMENT_RADIUS - 10}
            width={120}
            height={20}
            rx={10}
            className="pill"
          />
          <text x={CENTER.x} y={CENTER.y - ENVIRONMENT_RADIUS + 4} textAnchor="middle">
            Relays
          </text>
        </g>
        <g>
          {[...topology.links.values()]
            .filter((link) => visible.links.has(link.key))
            .map((link) => {
              const d = linkPath(topology, layout, link)
              if (!d) return null
              const color = focusedLinks.has(link.key) ? 'focus' : linkColorClass(link)
              const mbps = linkMbps(link, visible.trackKeys)
              const duration = Math.min(20, Math.max(0.3, 3 / Math.max(mbps, 0.05)))
              const classes = [
                'flow',
                color,
                selection?.kind === 'link' && selection.id === link.key ? 'sel' : '',
                focusedLinks.has(link.key) ? 'focus' : '',
                !showsWholeMesh && !activeLinks.has(link.key) ? 'dim' : ''
              ]
              return (
                <path
                  key={link.key}
                  d={d}
                  className={classes.filter(Boolean).join(' ')}
                  markerEnd={`url(#arrow-${color})`}
                  style={{ animationDuration: `${duration.toFixed(2)}s` }}
                />
              )
            })}
        </g>
        <g>
          {[...visible.links].map((key) => {
            const link = topology.links.get(key)
            const d = link && linkPath(topology, layout, link)
            if (!d) return null
            return (
              <path
                key={key}
                d={d}
                className="hit"
                onClick={(event) => {
                  event.stopPropagation()
                  select({ kind: 'link', id: key })
                }}
              />
            )
          })}
        </g>
        <g>
          {topology.relays.map((relay) => {
            const point = layout.relays.get(relay.id)
            if (!point) return null
            const { w, h } = RELAY_HALF
            const selected = selection?.kind === 'relay' && selection.id === relay.id
            return (
              <g
                key={relay.id}
                className={`node relay${selected ? ' sel' : ''}${dimmed(activeNodes.has(relay.id))}`}
                onClick={(event) => {
                  event.stopPropagation()
                  select({ kind: 'relay', id: relay.id })
                }}
              >
                <rect className="box" x={point.x - w} y={point.y - h} width={w * 2} height={h * 2} rx={6} />
                {[0, 1].map((unit) => {
                  const y = point.y - h + 8 + unit * 17
                  return (
                    <g key={unit}>
                      <rect className="unit" x={point.x - w + 8} y={y} width={w * 2 - 16} height={13} rx={2} />
                      {[0, 1, 2, 3, 4, 5].map((vent) => (
                        <line
                          key={vent}
                          className="vent"
                          x1={point.x - w + 16 + vent * 6}
                          y1={y + 3}
                          x2={point.x - w + 16 + vent * 6}
                          y2={y + 10}
                        />
                      ))}
                      <circle className={`led led-${unit}`} cx={point.x + w - 24} cy={y + 6.5} r={2.2} />
                      <circle className="led-off" cx={point.x + w - 16} cy={y + 6.5} r={2.2} />
                    </g>
                  )
                })}
                <text x={point.x} y={point.y + 8} textAnchor="middle">
                  {relay.id}
                </text>
                <text x={point.x} y={point.y + 24} textAnchor="middle" className="muted">
                  RSS {formatMegabytes(relay.snapshot.process.rss_bytes)}
                </text>
                <text x={point.x} y={point.y + 38} textAnchor="middle" className="muted">
                  cache {formatMegabytes(relay.snapshot.process.cache_payload_bytes)}
                </text>
              </g>
            )
          })}
          {[...topology.clients.values()]
            .filter((client) => visible.clients.has(client.key))
            .map((client) => {
              const point = layout.clients.get(client.key)
              if (!point) return null
              const selected = selection?.kind === 'client' && selection.id === client.key
              return (
                <g
                  key={client.key}
                  className={`node${selected ? ' sel' : ''}${dimmed(activeNodes.has(client.key))}`}
                  transform={`translate(${point.x - CLIENT_HALF.w},${point.y - CLIENT_HALF.h})`}
                  onClick={(event) => {
                    event.stopPropagation()
                    select({ kind: 'client', id: client.key })
                  }}
                >
                  <rect className="box" width={CLIENT_HALF.w * 2} height={CLIENT_HALF.h * 2} rx={6} />
                  <text x={10} y={18}>
                    {truncated(client.label)}
                  </text>
                  <text x={10} y={33} className="muted">
                    {truncated(
                      labelOf(client.published.length ? client.published : client.subscribed),
                      client.trackCount > 0 ? MAX_LABEL_CHARS_BESIDE_BADGE : MAX_LABEL_CHARS
                    )}
                  </text>
                  {client.trackCount > 0 && (
                    <g>
                      <circle cx={86} cy={29} r={8} className="badge" />
                      <text x={86} y={33} textAnchor="middle" className="badge-text">
                        {client.trackCount}
                      </text>
                    </g>
                  )}
                </g>
              )
            })}
        </g>
      </svg>
      <div className="zoom" role="group" aria-label="Zoom">
        <button aria-label="Zoom in" onClick={() => zoomFromCenter(1 / ZOOM_STEP)}>
          +
        </button>
        <button aria-label="Zoom out" onClick={() => zoomFromCenter(ZOOM_STEP)}>
          −
        </button>
        <button aria-label="Fit to view" title="Fit to view" onClick={fit}>
          ⤢
        </button>
      </div>
    </div>
  )
}
