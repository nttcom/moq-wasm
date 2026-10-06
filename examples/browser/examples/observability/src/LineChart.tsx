export interface ChartLine {
  label: string
  color: string
  t: number[]
  v: (number | null)[]
}

interface Props {
  title: string
  unit: string
  lines: ChartLine[]
  fromMs: number
  toMs: number
  hoverMs: number | null
  onHover: (ms: number | null) => void
  markMs: number | null
  onPick: (ms: number) => void
}

const WIDTH = 300
const HEIGHT = 120
const MARGIN = { left: 36, right: 6, top: 6, bottom: 18 }
const PLOT_WIDTH = WIDTH - MARGIN.left - MARGIN.right
const PLOT_HEIGHT = HEIGHT - MARGIN.top - MARGIN.bottom
const DAY_MS = 86_400_000

function niceMax(value: number): number {
  if (value <= 0) return 1
  const magnitude = 10 ** Math.floor(Math.log10(value))
  return [1, 2, 2.5, 5, 10].map((factor) => factor * magnitude).find((candidate) => candidate >= value) ?? value
}

export function formatValue(value: number | null | undefined): string {
  if (value === null || value === undefined) return '—'
  if (value === 0) return '0'
  if (Math.abs(value) >= 100) return Math.round(value).toLocaleString()
  return Math.abs(value) >= 10 ? value.toFixed(1) : value.toFixed(2)
}

function timeLabel(ms: number, spanMs: number): string {
  const date = new Date(ms)
  const hhmm = `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`
  return spanMs >= DAY_MS ? `${date.getMonth() + 1}/${date.getDate()} ${hhmm}` : hhmm
}

function valueAt(line: ChartLine, ms: number | null): number | null {
  if (line.t.length === 0) return null
  if (ms === null) {
    for (let index = line.v.length - 1; index >= 0; index--) if (line.v[index] !== null) return line.v[index]
    return null
  }
  let best: number | null = null
  for (let index = 0; index < line.t.length && line.t[index] <= ms; index++) best = line.v[index]
  return best
}

export function LineChart({ title, unit, lines, fromMs, toMs, hoverMs, onHover, markMs, onPick }: Props) {
  const span = Math.max(1, toMs - fromMs)
  const values = lines.flatMap((line) => line.v.filter((value): value is number => value !== null))
  const max = niceMax(Math.max(0, ...values) * 1.1)
  const xOf = (ms: number) => MARGIN.left + ((ms - fromMs) / span) * PLOT_WIDTH
  const yOf = (value: number) => MARGIN.top + (1 - value / max) * PLOT_HEIGHT
  const segments = (line: ChartLine) => {
    const result: string[] = []
    let current: string[] = []
    line.t.forEach((ms, index) => {
      const value = line.v[index]
      if (value === null) {
        if (current.length) result.push(current.join(' '))
        current = []
      } else {
        current.push(`${xOf(ms).toFixed(1)},${yOf(value).toFixed(1)}`)
      }
    })
    if (current.length) result.push(current.join(' '))
    return result
  }

  const timeAt = (event: React.MouseEvent<SVGRectElement>): number | null => {
    const matrix = event.currentTarget.ownerSVGElement?.getScreenCTM()
    if (!matrix) return null
    const point = new DOMPoint(event.clientX, event.clientY).matrixTransform(matrix.inverse())
    const ratio = Math.min(1, Math.max(0, (point.x - MARGIN.left) / PLOT_WIDTH))
    return fromMs + ratio * span
  }
  const markX = markMs === null ? null : Math.min(xOf(toMs), Math.max(xOf(fromMs), xOf(markMs)))

  return (
    <div className="chart">
      <div className="chart-head">
        <span>{title}</span>
        <small>{unit}</small>
      </div>
      <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} className="plot">
        {[0, 0.5, 1].map((fraction) => (
          <g key={fraction}>
            <line
              x1={MARGIN.left}
              x2={WIDTH - MARGIN.right}
              y1={yOf(max * fraction)}
              y2={yOf(max * fraction)}
              className="grid"
            />
            <text x={MARGIN.left - 4} y={yOf(max * fraction) + 3} textAnchor="end" className="axis">
              {formatValue(max * fraction)}
            </text>
          </g>
        ))}
        {[0, 1 / 3, 2 / 3, 1].map((fraction) => (
          <text
            key={fraction}
            x={MARGIN.left + fraction * PLOT_WIDTH}
            y={HEIGHT - 4}
            textAnchor={fraction === 0 ? 'start' : fraction === 1 ? 'end' : 'middle'}
            className="axis"
          >
            {timeLabel(fromMs + fraction * span, span)}
          </text>
        ))}
        {lines.map((line) =>
          segments(line).map((points, index) => (
            <polyline
              key={`${line.label}-${index}`}
              points={points}
              fill="none"
              stroke={line.color}
              strokeWidth={1.5}
            />
          ))
        )}
        {markX !== null && <line x1={markX} x2={markX} y1={MARGIN.top} y2={HEIGHT - MARGIN.bottom} className="mark" />}
        {hoverMs !== null && (
          <line x1={xOf(hoverMs)} x2={xOf(hoverMs)} y1={MARGIN.top} y2={HEIGHT - MARGIN.bottom} className="cross" />
        )}
        <rect
          x={MARGIN.left}
          y={MARGIN.top}
          width={PLOT_WIDTH}
          height={PLOT_HEIGHT}
          className="hover"
          onMouseMove={(event) => onHover(timeAt(event))}
          onMouseLeave={() => onHover(null)}
          onClick={(event) => {
            const ms = timeAt(event)
            if (ms !== null) onPick(ms)
          }}
        />
      </svg>
      <div className="chart-legend">
        {lines.map((line) => (
          <span key={line.label}>
            <i style={{ background: line.color }} />
            {line.label}
            <b>{formatValue(valueAt(line, hoverMs))}</b>
          </span>
        ))}
      </div>
    </div>
  )
}
