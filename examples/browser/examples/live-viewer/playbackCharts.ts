export const SAMPLE_INTERVAL_MS = 500
const WINDOW_SAMPLES = 120
const HEIGHT = 84
const WIDE_HEIGHT = 132
const TOP = 8
const BOTTOM = 8
const LEFT = 44
const RIGHT = 10
const LABEL_GUTTER = 124
const LABEL_GAP = 14
const END_DOT_RADIUS = 4
const TOOLTIP_HEIGHT = 18
const TOOLTIP_PADDING = 6
const TOOLTIP_OFFSET = 8
const MILLIS_PER_SECOND = 1_000

export type PlaybackSample = {
  delayMs?: number
  bufferMs?: number
  targetMs?: number
  outputLatencyMs?: number
  spreadMs?: number
  kbps?: number
  syncMs?: number
}

type Key = keyof PlaybackSample

/// A band is stacked on the bands listed before it; a line is drawn on its
/// own value.
type SeriesSpec = { key: Key; label: string; color: string; band?: boolean }

type ChartSpec = {
  title: string
  unit: string
  headline: Key
  series: SeriesSpec[]
  signed?: boolean
  wide?: boolean
}

/// `hoveredSlot` is a position on the time axis, so a pointer held still
/// stays on the same spot while new samples push the old ones left.
type Chart = { spec: ChartSpec; svg: SVGSVGElement; value: HTMLElement; hoveredSlot?: number }

type Frame = {
  width: number
  height: number
  right: number
  x: (index: number) => number
  y: (value: number) => number
}

const CHARTS: ChartSpec[] = [
  {
    title: 'Buffer',
    unit: 'ms',
    headline: 'bufferMs',
    series: [
      { key: 'outputLatencyMs', label: 'output latency', color: 'var(--series-3)', band: true },
      { key: 'spreadMs', label: 'audio jitter (p-p)', color: 'var(--series-4)', band: true },
      { key: 'targetMs', label: 'target', color: 'var(--series-2)' },
      { key: 'bufferMs', label: 'buffer', color: 'var(--series-1)' }
    ],
    wide: true
  },
  {
    title: 'Delay',
    unit: 'ms',
    headline: 'delayMs',
    series: [{ key: 'delayMs', label: 'delay', color: 'var(--series-1)' }]
  },
  {
    title: 'Bitrate',
    unit: 'kbps',
    headline: 'kbps',
    series: [{ key: 'kbps', label: 'bitrate', color: 'var(--series-1)' }]
  },
  {
    title: 'A/V',
    unit: 'ms',
    headline: 'syncMs',
    series: [{ key: 'syncMs', label: 'A/V', color: 'var(--series-1)' }],
    signed: true
  }
]

/// Small multiples of the playback measurements over the last minute, one
/// scale each, sampled every `SAMPLE_INTERVAL_MS`. The buffer chart stacks
/// what its target is made of, output latency and peak-to-peak audio jitter, under the
/// target the bounds clamp it to and the buffer actually held. A missing
/// measurement leaves a gap.
export class PlaybackCharts {
  private readonly samples: PlaybackSample[] = []
  private readonly charts: Chart[]

  constructor(container: HTMLElement) {
    this.charts = CHARTS.map((spec) => this.buildChart(container, spec))
  }

  push(sample: PlaybackSample): void {
    this.samples.push(sample)
    if (this.samples.length > WINDOW_SAMPLES) {
      this.samples.shift()
    }
    this.render()
  }

  reset(): void {
    this.samples.length = 0
    this.render()
  }

  private buildChart(container: HTMLElement, spec: ChartSpec): Chart {
    const figure = document.createElement('figure')
    figure.className = spec.wide ? 'playback-chart wide' : 'playback-chart'
    const header = document.createElement('figcaption')
    const title = document.createElement('span')
    title.className = 'playback-chart-title'
    title.textContent = `${spec.title} (${spec.unit})`
    const value = document.createElement('span')
    value.className = 'playback-chart-value'
    header.append(title, value)
    if (spec.series.length > 1) {
      const legend = document.createElement('span')
      legend.className = 'playback-chart-legend'
      legend.innerHTML = spec.series.map(legendItem).join('')
      header.append(legend)
    }
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
    svg.setAttribute('role', 'img')
    svg.setAttribute('aria-label', `${spec.title} over the last minute`)
    svg.style.height = `${spec.wide ? WIDE_HEIGHT : HEIGHT}px`
    figure.append(header, svg)
    container.append(figure)
    const chart: Chart = { spec, svg, value }
    svg.addEventListener('pointermove', (event) => {
      chart.hoveredSlot = slotAt(chart, event.clientX)
      this.renderChart(chart)
    })
    svg.addEventListener('pointerleave', () => {
      chart.hoveredSlot = undefined
      this.renderChart(chart)
    })
    return chart
  }

  private render(): void {
    for (const chart of this.charts) {
      this.renderChart(chart)
    }
  }

  private renderChart(chart: Chart): void {
    const { spec, svg } = chart
    const latest = this.samples.at(-1)?.[spec.headline]
    chart.value.textContent = latest === undefined ? '-' : formatValue(latest, spec)
    const width = Math.round(svg.getBoundingClientRect().width)
    if (width === 0) {
      return
    }
    const height = spec.wide ? WIDE_HEIGHT : HEIGHT
    const right = spec.wide ? LABEL_GUTTER : RIGHT
    svg.setAttribute('viewBox', `0 0 ${width} ${height}`)
    const top = scaleTop(this.samples, spec)
    const bottom = spec.signed ? -top : 0
    const plotHeight = height - TOP - BOTTOM
    const plotWidth = width - LEFT - right
    const frame: Frame = {
      width,
      height,
      right,
      x: (index) => LEFT + ((index + WINDOW_SAMPLES - this.samples.length) / (WINDOW_SAMPLES - 1)) * plotWidth,
      y: (value) => TOP + ((top - value) / (top - bottom)) * plotHeight
    }
    const parts = gridAndTicks(frame, top, bottom)
    const stacks = stackBands(this.samples, spec.series)
    for (const series of spec.series.filter((series) => series.band)) {
      parts.push(bandPath(stacks.get(series.key)!, frame, series.color))
    }
    for (const series of spec.series.filter((series) => !series.band)) {
      parts.push(
        `<path class="playback-chart-line" stroke="${series.color}" d="${linePath(this.samples, series.key, frame)}"/>`
      )
      const end = lastDefined(this.samples, series.key)
      if (end) {
        parts.push(dot(frame.x(end.index), frame.y(end.value), series.color))
      }
    }
    if (spec.wide) {
      parts.push(directLabels(this.samples, spec.series, stacks, frame))
    }
    const hovered = chart.hoveredSlot === undefined ? -1 : chart.hoveredSlot - (WINDOW_SAMPLES - this.samples.length)
    if (this.samples[hovered]) {
      parts.push(this.hoverLayer(chart, hovered, stacks, frame))
    }
    svg.innerHTML = parts.join('')
    placeTooltip(svg, width - right)
  }

  private hoverLayer(chart: Chart, index: number, stacks: Map<Key, Stack>, frame: Frame): string {
    const sample = this.samples[index]
    const at = frame.x(index)
    const agoSeconds = ((this.samples.length - 1 - index) * SAMPLE_INTERVAL_MS) / MILLIS_PER_SECOND
    const values = [...chart.spec.series]
      .reverse()
      .map((series) => {
        const value = sample[series.key]
        return value === undefined ? undefined : `${series.label} ${formatValue(value, chart.spec)}`
      })
      .filter((text) => text !== undefined)
    const text = `−${agoSeconds.toFixed(1)} s · ${values.length > 0 ? values.join(' · ') : '-'}`
    const dots = chart.spec.series.map((series) => {
      const value = series.band ? stacks.get(series.key)?.[index]?.top : sample[series.key]
      return value === undefined ? '' : dot(at, frame.y(value), series.color)
    })
    return [
      `<line class="playback-chart-crosshair" x1="${at}" x2="${at}" y1="${TOP}" y2="${frame.height - BOTTOM}"/>`,
      ...dots,
      `<rect class="playback-chart-tooltip" data-at="${at}" y="${TOP}" height="${TOOLTIP_HEIGHT}" rx="4"/>`,
      `<text class="playback-chart-tooltip-text" y="${TOP + 13}">${text}</text>`
    ].join('')
  }
}

function slotAt(chart: Chart, clientX: number): number {
  const bounds = chart.svg.getBoundingClientRect()
  const plotWidth = bounds.width - LEFT - (chart.spec.wide ? LABEL_GUTTER : RIGHT)
  return Math.round(((clientX - bounds.left - LEFT) / plotWidth) * (WINDOW_SAMPLES - 1))
}

type Stack = ({ base: number; top: number } | undefined)[]

function stackBands(samples: PlaybackSample[], series: SeriesSpec[]): Map<Key, Stack> {
  const stacks = new Map<Key, Stack>()
  const bands = series.filter((spec) => spec.band)
  for (const [position, band] of bands.entries()) {
    stacks.set(
      band.key,
      samples.map((sample) => {
        const below = bands.slice(0, position).map((lower) => sample[lower.key])
        const value = sample[band.key]
        if (value === undefined || below.some((lower) => lower === undefined)) {
          return undefined
        }
        const base = below.reduce<number>((sum, lower) => sum + (lower ?? 0), 0)
        return { base, top: base + value }
      })
    )
  }
  return stacks
}

function gridAndTicks(frame: Frame, top: number, bottom: number): string[] {
  const line = (value: number) =>
    `<line class="playback-chart-grid" x1="${LEFT}" x2="${frame.width - frame.right}" y1="${frame.y(value)}" y2="${frame.y(value)}"/>`
  const tick = (value: number) =>
    `<text class="playback-chart-tick" x="${LEFT - 6}" y="${frame.y(value) + 4}">${Math.round(value).toLocaleString()}</text>`
  const values = bottom < 0 ? [top, 0, bottom] : [top, 0]
  return values.flatMap((value) => [line(value), tick(value)])
}

/// Each contiguous run becomes one closed shape, outlined in the surface
/// colour so the band above it stays apart.
function bandPath(stack: Stack, frame: Frame, color: string): string {
  const runs: { index: number; base: number; top: number }[][] = [[]]
  stack.forEach((point, index) => {
    if (point) {
      runs.at(-1)!.push({ index, ...point })
    } else if (runs.at(-1)!.length > 0) {
      runs.push([])
    }
  })
  return runs
    .filter((run) => run.length > 0)
    .map((run) => {
      const upper = run.map((point) => `${frame.x(point.index).toFixed(1)},${frame.y(point.top).toFixed(1)}`)
      const lower = [...run]
        .reverse()
        .map((point) => `${frame.x(point.index).toFixed(1)},${frame.y(point.base).toFixed(1)}`)
      return `<path class="playback-chart-band" fill="${color}" d="M${[...upper, ...lower].join('L')}Z"/>`
    })
    .join('')
}

/// Labels the end of every series in the gutter, nudged apart so none
/// overlaps its neighbour.
function directLabels(samples: PlaybackSample[], series: SeriesSpec[], stacks: Map<Key, Stack>, frame: Frame): string {
  const placed = series
    .map((spec) => {
      if (spec.band) {
        const end = stacks.get(spec.key)?.findLast((point) => point !== undefined)
        return end && { spec, y: frame.y((end.base + end.top) / 2) }
      }
      const end = lastDefined(samples, spec.key)
      return end && { spec, y: frame.y(end.value) }
    })
    .filter((label) => label !== undefined)
    .sort((left, right) => left.y - right.y)
  for (let i = 1; i < placed.length; i += 1) {
    placed[i].y = Math.max(placed[i].y, placed[i - 1].y + LABEL_GAP)
  }
  const overflow = (placed.at(-1)?.y ?? 0) - (frame.height - BOTTOM)
  if (overflow > 0) {
    for (const label of placed) {
      label.y -= overflow
    }
  }
  const x = frame.width - frame.right + END_DOT_RADIUS + 6
  return placed
    .map(({ spec, y }) => `<text class="playback-chart-label" x="${x}" y="${y + 4}">${spec.label}</text>`)
    .join('')
}

/// The tooltip is sized from its rendered text and flips to the left of the
/// crosshair when it would run past the plot.
function placeTooltip(svg: SVGSVGElement, plotRight: number): void {
  const box = svg.querySelector<SVGRectElement>('.playback-chart-tooltip')
  const label = svg.querySelector<SVGTextElement>('.playback-chart-tooltip-text')
  if (!box || !label) {
    return
  }
  const at = Number(box.dataset.at)
  const boxWidth = label.getComputedTextLength() + 2 * TOOLTIP_PADDING
  const left = at + TOOLTIP_OFFSET + boxWidth > plotRight ? at - TOOLTIP_OFFSET - boxWidth : at + TOOLTIP_OFFSET
  box.setAttribute('x', String(Math.max(0, left)))
  box.setAttribute('width', String(boxWidth))
  label.setAttribute('x', String(Math.max(0, left) + TOOLTIP_PADDING))
}

function legendItem(series: SeriesSpec): string {
  const key = series.band
    ? `<rect x="1" y="1" width="12" height="8" rx="2" fill="${series.color}" fill-opacity="0.35"/>`
    : `<line x1="1" y1="5" x2="13" y2="5" stroke="${series.color}" stroke-width="2" stroke-linecap="round"/>`
  return `<span><svg width="14" height="10" aria-hidden="true">${key}</svg>${series.label}</span>`
}

function scaleTop(samples: PlaybackSample[], spec: ChartSpec): number {
  let largest = 0
  const bands = spec.series.filter((series) => series.band)
  for (const sample of samples) {
    largest = Math.max(
      largest,
      bands.reduce((sum, band) => sum + (sample[band.key] ?? 0), 0)
    )
    for (const series of spec.series.filter((series) => !series.band)) {
      largest = Math.max(largest, Math.abs(sample[series.key] ?? 0))
    }
  }
  return niceCeil(Math.max(largest, spec.signed ? 10 : 1))
}

function niceCeil(value: number): number {
  const magnitude = 10 ** Math.floor(Math.log10(value))
  const step = [1, 2, 5, 10].find((candidate) => candidate * magnitude >= value) ?? 10
  return step * magnitude
}

function linePath(samples: PlaybackSample[], key: Key, frame: Frame): string {
  let path = ''
  let drawing = false
  samples.forEach((sample, index) => {
    const value = sample[key]
    if (value === undefined) {
      drawing = false
      return
    }
    path += `${drawing ? 'L' : 'M'}${frame.x(index).toFixed(1)},${frame.y(value).toFixed(1)}`
    drawing = true
  })
  return path
}

function lastDefined(samples: PlaybackSample[], key: Key): { index: number; value: number } | undefined {
  const index = samples.findLastIndex((sample) => sample[key] !== undefined)
  return index < 0 ? undefined : { index, value: samples[index][key]! }
}

function dot(cx: number, cy: number, color: string): string {
  return `<circle class="playback-chart-dot" cx="${cx}" cy="${cy}" r="${END_DOT_RADIUS}" fill="${color}"/>`
}

function formatValue(value: number, spec: ChartSpec): string {
  const rounded = Math.round(value)
  return `${spec.signed && rounded > 0 ? '+' : ''}${rounded.toLocaleString()} ${spec.unit}`
}
