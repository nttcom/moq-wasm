export const SAMPLE_INTERVAL_MS = 500
const WINDOW_SAMPLES = 120
const HEIGHT = 84
const TOP = 8
const BOTTOM = 8
const LEFT = 44
const RIGHT = 10
const END_DOT_RADIUS = 4
const TOOLTIP_HEIGHT = 18
const TOOLTIP_PADDING = 6
const TOOLTIP_OFFSET = 8
const MILLIS_PER_SECOND = 1_000

export type PlaybackSample = {
  delayMs?: number
  bufferMs?: number
  targetMs?: number
  kbps?: number
  syncMs?: number
}

type SeriesSpec = { key: keyof PlaybackSample; label: string; color: string }

type ChartSpec = {
  title: string
  unit: string
  series: SeriesSpec[]
  signed?: boolean
}

type Chart = { spec: ChartSpec; svg: SVGSVGElement; value: HTMLElement; hovered?: number }

const CHARTS: ChartSpec[] = [
  { title: 'Delay', unit: 'ms', series: [{ key: 'delayMs', label: 'delay', color: 'var(--series-1)' }] },
  {
    title: 'Buffer',
    unit: 'ms',
    series: [
      { key: 'bufferMs', label: 'buffer', color: 'var(--series-1)' },
      { key: 'targetMs', label: 'target', color: 'var(--series-2)' }
    ]
  },
  { title: 'Bitrate', unit: 'kbps', series: [{ key: 'kbps', label: 'bitrate', color: 'var(--series-1)' }] },
  {
    title: 'A/V',
    unit: 'ms',
    series: [{ key: 'syncMs', label: 'A/V', color: 'var(--series-1)' }],
    signed: true
  }
]

/// Small multiples of the playback measurements over the last minute, one
/// scale each, sampled every `SAMPLE_INTERVAL_MS`. A missing measurement
/// leaves a gap in its line.
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
    figure.className = 'playback-chart'
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
      legend.innerHTML = spec.series
        .map(
          (series) =>
            `<span><svg width="14" height="8" aria-hidden="true"><line x1="1" y1="4" x2="13" y2="4" stroke="${series.color}" stroke-width="2" stroke-linecap="round"/></svg>${series.label}</span>`
        )
        .join('')
      header.append(legend)
    }
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
    svg.setAttribute('role', 'img')
    svg.setAttribute('aria-label', `${spec.title} over the last minute`)
    figure.append(header, svg)
    container.append(figure)
    const chart: Chart = { spec, svg, value }
    svg.addEventListener('pointermove', (event) => {
      chart.hovered = this.indexAt(svg, event.clientX)
      this.renderChart(chart)
    })
    svg.addEventListener('pointerleave', () => {
      chart.hovered = undefined
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
    const [main] = spec.series
    const latest = this.samples.at(-1)?.[main.key]
    chart.value.textContent = latest === undefined ? '-' : formatValue(latest, spec)
    const width = Math.round(svg.getBoundingClientRect().width)
    if (width === 0) {
      return
    }
    svg.setAttribute('viewBox', `0 0 ${width} ${HEIGHT}`)
    const plotWidth = width - LEFT - RIGHT
    const plotHeight = HEIGHT - TOP - BOTTOM
    const top = scaleTop(this.samples, spec)
    const bottom = spec.signed ? -top : 0
    const y = (value: number) => TOP + ((top - value) / (top - bottom)) * plotHeight
    const x = (index: number) =>
      LEFT + ((index + WINDOW_SAMPLES - this.samples.length) / (WINDOW_SAMPLES - 1)) * plotWidth
    const parts = [
      `<line class="playback-chart-grid" x1="${LEFT}" x2="${width - RIGHT}" y1="${y(top)}" y2="${y(top)}"/>`,
      `<line class="playback-chart-grid" x1="${LEFT}" x2="${width - RIGHT}" y1="${y(0)}" y2="${y(0)}"/>`,
      `<text class="playback-chart-tick" x="${LEFT - 6}" y="${y(top) + 4}">${formatTick(top)}</text>`,
      `<text class="playback-chart-tick" x="${LEFT - 6}" y="${y(0) + 4}">0</text>`
    ]
    if (spec.signed) {
      parts.push(
        `<line class="playback-chart-grid" x1="${LEFT}" x2="${width - RIGHT}" y1="${y(bottom)}" y2="${y(bottom)}"/>`,
        `<text class="playback-chart-tick" x="${LEFT - 6}" y="${y(bottom) + 4}">${formatTick(bottom)}</text>`
      )
    }
    for (const series of [...spec.series].reverse()) {
      parts.push(
        `<path class="playback-chart-line" stroke="${series.color}" d="${linePath(this.samples, series.key, x, y)}"/>`
      )
      const end = lastDefined(this.samples, series.key)
      if (end) {
        parts.push(dot(x(end.index), y(end.value), series.color))
      }
    }
    if (chart.hovered !== undefined && this.samples[chart.hovered]) {
      parts.push(this.hoverLayer(chart, chart.hovered, x, y))
    }
    svg.innerHTML = parts.join('')
    placeTooltip(svg, width)
  }

  private hoverLayer(chart: Chart, index: number, x: (index: number) => number, y: (value: number) => number): string {
    const sample = this.samples[index]
    const at = x(index)
    const agoSeconds = ((this.samples.length - 1 - index) * SAMPLE_INTERVAL_MS) / MILLIS_PER_SECOND
    const values = chart.spec.series
      .map((series) => {
        const value = sample[series.key]
        return value === undefined ? undefined : `${series.label} ${formatValue(value, chart.spec)}`
      })
      .filter((text) => text !== undefined)
    const text = `−${agoSeconds.toFixed(1)} s · ${values.length > 0 ? values.join(' · ') : '-'}`
    const dots = chart.spec.series.map((series) => {
      const value = sample[series.key]
      return value === undefined ? '' : dot(at, y(value), series.color)
    })
    return [
      `<line class="playback-chart-crosshair" x1="${at}" x2="${at}" y1="${TOP}" y2="${HEIGHT - BOTTOM}"/>`,
      ...dots,
      `<rect class="playback-chart-tooltip" data-at="${at}" y="${TOP}" height="${TOOLTIP_HEIGHT}" rx="4"/>`,
      `<text class="playback-chart-tooltip-text" y="${TOP + 13}">${text}</text>`
    ].join('')
  }

  private indexAt(svg: SVGSVGElement, clientX: number): number | undefined {
    const bounds = svg.getBoundingClientRect()
    const plotWidth = bounds.width - LEFT - RIGHT
    const slot = Math.round(((clientX - bounds.left - LEFT) / plotWidth) * (WINDOW_SAMPLES - 1))
    const index = slot - (WINDOW_SAMPLES - this.samples.length)
    return index >= 0 && index < this.samples.length ? index : undefined
  }
}

/// The tooltip is sized from its rendered text and flips to the left of the
/// crosshair when it would run past the plot.
function placeTooltip(svg: SVGSVGElement, width: number): void {
  const box = svg.querySelector<SVGRectElement>('.playback-chart-tooltip')
  const label = svg.querySelector<SVGTextElement>('.playback-chart-tooltip-text')
  if (!box || !label) {
    return
  }
  const at = Number(box.dataset.at)
  const boxWidth = label.getComputedTextLength() + 2 * TOOLTIP_PADDING
  const left = at + TOOLTIP_OFFSET + boxWidth > width - RIGHT ? at - TOOLTIP_OFFSET - boxWidth : at + TOOLTIP_OFFSET
  box.setAttribute('x', String(left))
  box.setAttribute('width', String(boxWidth))
  label.setAttribute('x', String(left + TOOLTIP_PADDING))
}

function scaleTop(samples: PlaybackSample[], spec: ChartSpec): number {
  let largest = 0
  for (const sample of samples) {
    for (const series of spec.series) {
      const value = sample[series.key]
      if (value !== undefined) {
        largest = Math.max(largest, Math.abs(value))
      }
    }
  }
  return niceCeil(Math.max(largest, spec.signed ? 10 : 1))
}

function niceCeil(value: number): number {
  const magnitude = 10 ** Math.floor(Math.log10(value))
  const step = [1, 2, 5, 10].find((candidate) => candidate * magnitude >= value) ?? 10
  return step * magnitude
}

function linePath(
  samples: PlaybackSample[],
  key: keyof PlaybackSample,
  x: (index: number) => number,
  y: (value: number) => number
): string {
  let path = ''
  let drawing = false
  samples.forEach((sample, index) => {
    const value = sample[key]
    if (value === undefined) {
      drawing = false
      return
    }
    path += `${drawing ? 'L' : 'M'}${x(index).toFixed(1)},${y(value).toFixed(1)}`
    drawing = true
  })
  return path
}

function lastDefined(
  samples: PlaybackSample[],
  key: keyof PlaybackSample
): { index: number; value: number } | undefined {
  for (let index = samples.length - 1; index >= 0; index -= 1) {
    const value = samples[index][key]
    if (value !== undefined) {
      return { index, value }
    }
  }
  return undefined
}

function dot(cx: number, cy: number, color: string): string {
  return `<circle class="playback-chart-dot" cx="${cx}" cy="${cy}" r="${END_DOT_RADIUS}" fill="${color}"/>`
}

function formatValue(value: number, spec: ChartSpec): string {
  const rounded = Math.round(value)
  return `${spec.signed && rounded > 0 ? '+' : ''}${rounded.toLocaleString()} ${spec.unit}`
}

function formatTick(value: number): string {
  return Math.round(value).toLocaleString()
}
