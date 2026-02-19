import type { SidebarStatsSample } from '../types/stats'

type ChartYAxisPadding = {
  topRatio: number
  bottomRatio: number
}
type MetricAccessor = (sample: SidebarStatsSample) => number | null | undefined
type StatsChartBaseConfig = {
  key: string
  title: string
  yTicks?: number[]
  yTickStep?: number
  adaptiveMaxTickCount?: number
  yPadding: ChartYAxisPadding
  yTickLabelFormatter: (value: number) => string
}
type SingleMetricChartConfig = StatsChartBaseConfig & {
  kind: 'single'
  accessor: MetricAccessor
  colorClass: string
}
type MultiMetricSeriesConfig = {
  label: string
  accessor: MetricAccessor
  colorClass: string
}
type MultiMetricChartConfig = StatsChartBaseConfig & {
  kind: 'multi'
  series: MultiMetricSeriesConfig[]
}
type StatsChartConfig = SingleMetricChartConfig | MultiMetricChartConfig

const CHART_VIEWBOX_HEIGHT = 48
const SINGLE_CHART_LINE_STROKE_WIDTH = 1.2
const MULTI_CHART_LINE_STROKE_WIDTH = 1.1
const AUDIO_BITRATE_Y_TICKS = [30, 60, 90, 120, 160, 200]
const RENDERING_RATE_Y_TICKS = [10, 20, 30, 40, 50, 60]
const VIDEO_BITRATE_TICK_STEP_KBPS = 250
const KEYFRAME_INTERVAL_TICK_STEP_FRAMES = 30
const KEYFRAME_INTERVAL_MAX_TICK_COUNT = 6
const LATENCY_Y_TICK_STEP_MS = 100
const AUDIO_PLAYOUT_QUEUE_TICK_STEP_MS = 10
const ADAPTIVE_TICK_HEADROOM_RATIO = 0.25
const BITRATE_Y_PADDING: ChartYAxisPadding = { topRatio: 0.14, bottomRatio: 0.1 }
const RENDERING_RATE_Y_PADDING: ChartYAxisPadding = { topRatio: 0.16, bottomRatio: 0.12 }
const LATENCY_Y_PADDING: ChartYAxisPadding = { topRatio: 0.12, bottomRatio: 0.1 }

const STATS_CHART_CONFIGS: StatsChartConfig[] = [
  {
    kind: 'single',
    key: 'video-bitrate',
    title: 'Video Bitrate (kbps)',
    accessor: (sample) => sample.videoBitrateKbps,
    colorClass: 'text-cyan-300',
    yTickStep: VIDEO_BITRATE_TICK_STEP_KBPS,
    yPadding: BITRATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} kbps`
  },
  {
    kind: 'single',
    key: 'audio-bitrate',
    title: 'Audio Bitrate (kbps)',
    accessor: (sample) => sample.audioBitrateKbps,
    colorClass: 'text-emerald-300',
    yTicks: AUDIO_BITRATE_Y_TICKS,
    yPadding: BITRATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} kbps`
  },
  {
    kind: 'single',
    key: 'audio-playout-queue',
    title: 'Audio Playout Queue',
    accessor: (sample) => sample.audioPlaybackQueueMs,
    colorClass: 'text-teal-300',
    yTickStep: AUDIO_PLAYOUT_QUEUE_TICK_STEP_MS,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    kind: 'single',
    key: 'screenshare-bitrate',
    title: 'ScreenShare Bitrate (kbps)',
    accessor: (sample) => sample.screenShareBitrateKbps,
    colorClass: 'text-orange-300',
    yTickStep: VIDEO_BITRATE_TICK_STEP_KBPS,
    yPadding: BITRATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} kbps`
  },
  {
    kind: 'single',
    key: 'video-frame-rate',
    title: 'Video frame rate/s',
    accessor: (sample) => sample.videoRenderingRateFps,
    colorClass: 'text-fuchsia-300',
    yTicks: RENDERING_RATE_Y_TICKS,
    yPadding: RENDERING_RATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} /s`
  },
  {
    kind: 'single',
    key: 'video-keyframe-interval',
    title: 'Video Keyframe Interval (frames)',
    accessor: (sample) => sample.videoKeyframeIntervalFrames,
    colorClass: 'text-lime-300',
    yTickStep: KEYFRAME_INTERVAL_TICK_STEP_FRAMES,
    adaptiveMaxTickCount: KEYFRAME_INTERVAL_MAX_TICK_COUNT,
    yPadding: RENDERING_RATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} frames`
  },
  {
    kind: 'single',
    key: 'screenshare-keyframe-interval',
    title: 'ScreenShare Keyframe Interval (frames)',
    accessor: (sample) => sample.screenShareKeyframeIntervalFrames,
    colorClass: 'text-yellow-300',
    yTickStep: KEYFRAME_INTERVAL_TICK_STEP_FRAMES,
    adaptiveMaxTickCount: KEYFRAME_INTERVAL_MAX_TICK_COUNT,
    yPadding: RENDERING_RATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} frames`
  },
  {
    kind: 'multi',
    key: 'video-latency',
    title: 'Video Latency',
    series: [
      {
        label: 'Network',
        accessor: (sample) => sample.videoReceiveLatencyMs,
        colorClass: 'text-amber-300'
      },
      {
        label: 'E2E',
        accessor: (sample) => sample.videoRenderLatencyMs,
        colorClass: 'text-rose-300'
      }
    ],
    yTickStep: LATENCY_Y_TICK_STEP_MS,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    kind: 'multi',
    key: 'audio-latency',
    title: 'Audio Latency',
    series: [
      {
        label: 'Network',
        accessor: (sample) => sample.audioReceiveLatencyMs,
        colorClass: 'text-sky-300'
      },
      {
        label: 'E2E',
        accessor: (sample) => sample.audioRenderLatencyMs,
        colorClass: 'text-violet-300'
      }
    ],
    yTickStep: LATENCY_Y_TICK_STEP_MS,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  }
]

export function MemberStatsCharts({
  samples,
  chartHeightClass = 'h-44'
}: {
  samples: SidebarStatsSample[]
  chartHeightClass?: string
}) {
  const visibleCharts = STATS_CHART_CONFIGS.filter((chart) =>
    chart.kind === 'single'
      ? hasMetricDataInSamples(samples, chart.accessor)
      : chart.series.some((series) => hasMetricDataInSamples(samples, series.accessor))
  )

  if (!visibleCharts.length) {
    return (
      <div className="rounded-md border border-white/10 bg-white/[0.03] p-4 text-xs text-blue-200">
        No subscribed track metrics for this member.
      </div>
    )
  }

  return (
    <div className="space-y-4">
      {visibleCharts.map((chart) => {
        const resolvedTicks = chart.yTickStep
          ? buildAdaptiveTicks(
              samples,
              chart.kind === 'single' ? [chart.accessor] : chart.series.map((series) => series.accessor),
              chart.yTickStep,
              chart.adaptiveMaxTickCount
            )
          : chart.yTicks
        if (chart.kind === 'single') {
          return (
            <MetricChart
              key={chart.key}
              title={chart.title}
              samples={samples}
              accessor={chart.accessor}
              colorClass={chart.colorClass}
              yTicks={resolvedTicks}
              yPadding={chart.yPadding}
              yTickLabelFormatter={chart.yTickLabelFormatter}
              chartHeightClass={chartHeightClass}
            />
          )
        }
        return (
          <MultiMetricChart
            key={chart.key}
            title={chart.title}
            samples={samples}
            series={chart.series}
            yTicks={resolvedTicks}
            yPadding={chart.yPadding}
            yTickLabelFormatter={chart.yTickLabelFormatter}
            chartHeightClass={chartHeightClass}
          />
        )
      })}
    </div>
  )
}

type MetricChartEntry = { index: number; value: number }

function buildMetricEntries(samples: SidebarStatsSample[], accessor: MetricAccessor): MetricChartEntry[] {
  return samples
    .map((sample, index) => ({
      index,
      value: accessor(sample)
    }))
    .filter((entry): entry is MetricChartEntry => typeof entry.value === 'number' && Number.isFinite(entry.value))
}

function MetricChart({
  title,
  samples,
  accessor,
  colorClass,
  yTicks,
  yPadding,
  yTickLabelFormatter,
  chartHeightClass
}: {
  title: string
  samples: SidebarStatsSample[]
  accessor: MetricAccessor
  colorClass: string
  yTicks?: number[]
  yPadding?: ChartYAxisPadding
  yTickLabelFormatter?: (value: number) => string
  chartHeightClass: string
}) {
  const entries = buildMetricEntries(samples, accessor)

  const dataValues = entries.map((entry) => entry.value)
  const tickValues = (yTicks ?? []).filter((value) => Number.isFinite(value))
  const min = dataValues.length
    ? Math.min(...dataValues, ...tickValues)
    : tickValues.length
      ? Math.min(...tickValues)
      : 0
  const max = dataValues.length
    ? Math.max(...dataValues, ...tickValues)
    : tickValues.length
      ? Math.max(...tickValues)
      : 0
  const topPaddingRatio = sanitizePaddingRatio(yPadding?.topRatio)
  const bottomPaddingRatio = sanitizePaddingRatio(yPadding?.bottomRatio)
  const rawMin = Number.isFinite(min) ? min : 0
  const rawMax = Number.isFinite(max) ? max : rawMin + 1
  const rawRange = Math.max(1, rawMax - rawMin)
  const axisMin = rawMin - rawRange * bottomPaddingRatio
  const axisMax = Math.max(axisMin + 1, rawMax + rawRange * topPaddingRatio)
  const denominator = Math.max(1, samples.length - 1)
  const toChartY = (value: number) => ((axisMax - value) / (axisMax - axisMin)) * CHART_VIEWBOX_HEIGHT
  const polyline = entries
    .map((entry) => {
      const x = (entry.index / denominator) * 100
      const y = toChartY(entry.value)
      return `${x.toFixed(2)},${y.toFixed(2)}`
    })
    .join(' ')

  return (
    <section className="rounded-lg border border-white/10 bg-white/[0.03] p-3">
      <h4 className={`mb-2 text-xs font-semibold ${colorClass}`}>{title}</h4>
      {entries.length >= 2 ? (
        <svg
          viewBox={`0 0 100 ${CHART_VIEWBOX_HEIGHT}`}
          className={`${chartHeightClass} w-full overflow-visible rounded bg-black/20`}
        >
          {tickValues.map((tick) => {
            const y = toChartY(tick)
            return (
              <g key={`${title}-tick-${tick}`}>
                <line x1={0} y1={y} x2={100} y2={y} stroke="rgba(148, 163, 184, 0.28)" strokeDasharray="1.2 1.2" />
                <text x={1.2} y={Math.max(2.5, y - 0.6)} fontSize={2.4} fill="rgba(191, 219, 254, 0.75)">
                  {yTickLabelFormatter ? yTickLabelFormatter(tick) : formatMetricValue(tick, 0)}
                </text>
              </g>
            )
          })}
          <polyline
            points={polyline}
            fill="none"
            stroke="currentColor"
            strokeWidth={SINGLE_CHART_LINE_STROKE_WIDTH}
            className={colorClass}
          />
        </svg>
      ) : (
        <div
          className={`flex ${chartHeightClass} items-center justify-center rounded bg-black/20 text-xs text-blue-200`}
        >
          No data
        </div>
      )}
    </section>
  )
}

function MultiMetricChart({
  title,
  samples,
  series,
  yTicks,
  yPadding,
  yTickLabelFormatter,
  chartHeightClass
}: {
  title: string
  samples: SidebarStatsSample[]
  series: MultiMetricSeriesConfig[]
  yTicks?: number[]
  yPadding?: ChartYAxisPadding
  yTickLabelFormatter?: (value: number) => string
  chartHeightClass: string
}) {
  const seriesEntries = series.map((seriesConfig) => ({
    ...seriesConfig,
    entries: buildMetricEntries(samples, seriesConfig.accessor)
  }))
  const hasRenderableSeries = seriesEntries.some((seriesConfig) => seriesConfig.entries.length >= 2)
  const dataValues = seriesEntries.flatMap((seriesConfig) => seriesConfig.entries.map((entry) => entry.value))
  const tickValues = (yTicks ?? []).filter((value) => Number.isFinite(value))
  const min = dataValues.length
    ? Math.min(...dataValues, ...tickValues)
    : tickValues.length
      ? Math.min(...tickValues)
      : 0
  const max = dataValues.length
    ? Math.max(...dataValues, ...tickValues)
    : tickValues.length
      ? Math.max(...tickValues)
      : 0
  const topPaddingRatio = sanitizePaddingRatio(yPadding?.topRatio)
  const bottomPaddingRatio = sanitizePaddingRatio(yPadding?.bottomRatio)
  const rawMin = Number.isFinite(min) ? min : 0
  const rawMax = Number.isFinite(max) ? max : rawMin + 1
  const rawRange = Math.max(1, rawMax - rawMin)
  const axisMin = rawMin - rawRange * bottomPaddingRatio
  const axisMax = Math.max(axisMin + 1, rawMax + rawRange * topPaddingRatio)
  const denominator = Math.max(1, samples.length - 1)
  const toChartY = (value: number) => ((axisMax - value) / (axisMax - axisMin)) * CHART_VIEWBOX_HEIGHT
  const toPolylinePoints = (entries: MetricChartEntry[]) =>
    entries
      .map((entry) => {
        const x = (entry.index / denominator) * 100
        const y = toChartY(entry.value)
        return `${x.toFixed(2)},${y.toFixed(2)}`
      })
      .join(' ')
  const toAreaPoints = (entries: MetricChartEntry[]) => {
    if (entries.length < 2) {
      return ''
    }
    const baselineY = CHART_VIEWBOX_HEIGHT.toFixed(2)
    const startX = ((entries[0].index / denominator) * 100).toFixed(2)
    const endX = ((entries[entries.length - 1].index / denominator) * 100).toFixed(2)
    return `${startX},${baselineY} ${toPolylinePoints(entries)} ${endX},${baselineY}`
  }
  const visibleSeries = seriesEntries.filter((seriesConfig) => seriesConfig.entries.length > 0)

  return (
    <section className="rounded-lg border border-white/10 bg-white/[0.03] p-3">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h4 className="text-xs font-semibold text-blue-100">{title}</h4>
        <div className="flex flex-wrap items-center gap-2 text-[10px]">
          {visibleSeries.map((seriesConfig) => (
            <div key={`${title}-${seriesConfig.label}`} className="inline-flex items-center gap-1.5 text-blue-100/90">
              <span className={`inline-block h-2 w-2 rounded-sm bg-current ${seriesConfig.colorClass}`} />
              <span>{seriesConfig.label}</span>
            </div>
          ))}
        </div>
      </div>
      {hasRenderableSeries ? (
        <svg
          viewBox={`0 0 100 ${CHART_VIEWBOX_HEIGHT}`}
          className={`${chartHeightClass} w-full overflow-visible rounded bg-black/20`}
        >
          {tickValues.map((tick) => {
            const y = toChartY(tick)
            return (
              <g key={`${title}-tick-${tick}`}>
                <line x1={0} y1={y} x2={100} y2={y} stroke="rgba(148, 163, 184, 0.28)" strokeDasharray="1.2 1.2" />
                <text x={1.2} y={Math.max(2.5, y - 0.6)} fontSize={2.4} fill="rgba(191, 219, 254, 0.75)">
                  {yTickLabelFormatter ? yTickLabelFormatter(tick) : formatMetricValue(tick, 0)}
                </text>
              </g>
            )
          })}
          {seriesEntries.map((seriesConfig) => {
            if (seriesConfig.entries.length < 2) {
              return null
            }
            return (
              <g key={`${title}-${seriesConfig.label}`} className={seriesConfig.colorClass}>
                <polygon points={toAreaPoints(seriesConfig.entries)} fill="currentColor" fillOpacity={0.16} />
                <polyline
                  points={toPolylinePoints(seriesConfig.entries)}
                  fill="none"
                  stroke="currentColor"
                  strokeWidth={MULTI_CHART_LINE_STROKE_WIDTH}
                />
              </g>
            )
          })}
        </svg>
      ) : (
        <div
          className={`flex ${chartHeightClass} items-center justify-center rounded bg-black/20 text-xs text-blue-200`}
        >
          No data
        </div>
      )}
    </section>
  )
}

function sanitizePaddingRatio(value: number | undefined): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return 0
  }
  return Math.max(0, value)
}

function hasMetricDataInSamples(samples: SidebarStatsSample[], accessor: MetricAccessor): boolean {
  return samples.some((sample) => {
    const value = accessor(sample)
    return typeof value === 'number' && Number.isFinite(value)
  })
}

function buildAdaptiveTicks(
  samples: SidebarStatsSample[],
  accessors: MetricAccessor[],
  step: number,
  maxTickCount?: number
): number[] {
  if (!Number.isFinite(step) || step <= 0) {
    return []
  }
  const values = accessors.flatMap((accessor) =>
    samples
      .map((sample) => accessor(sample))
      .filter((value): value is number => typeof value === 'number' && Number.isFinite(value))
  )
  if (!values.length) {
    return [step]
  }
  const maxValue = Math.max(...values)
  const maxWithHeadroom = maxValue * (1 + ADAPTIVE_TICK_HEADROOM_RATIO)
  const resolvedStep = resolveAdaptiveStep(step, maxWithHeadroom, maxTickCount)
  const maxTick = Math.max(resolvedStep, Math.ceil(maxWithHeadroom / resolvedStep) * resolvedStep)
  const ticks: number[] = []
  for (let tick = resolvedStep; tick <= maxTick; tick += resolvedStep) {
    ticks.push(tick)
  }
  return ticks
}

function resolveAdaptiveStep(baseStep: number, maxWithHeadroom: number, maxTickCount?: number): number {
  if (!Number.isFinite(maxTickCount) || typeof maxTickCount !== 'number' || maxTickCount <= 0) {
    return baseStep
  }
  const requiredStep = maxWithHeadroom / maxTickCount
  if (!Number.isFinite(requiredStep) || requiredStep <= baseStep) {
    return baseStep
  }
  return Math.ceil(requiredStep / baseStep) * baseStep
}

function formatMetricValue(value: number | null | undefined, fractionDigits: number): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return '-'
  }
  return value.toFixed(fractionDigits)
}
