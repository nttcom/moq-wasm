import { useState } from 'react'
import type { SidebarStatsSample } from '../types/stats'

type ChartYAxisPadding = {
  topRatio: number
  bottomRatio: number
}
type MetricAccessor = (sample: SidebarStatsSample) => number | null | undefined
type MetricSeriesConfig = {
  label: string
  accessor: MetricAccessor
  colorClass: string
}
type StatsChartConfig = {
  key: string
  title: string
  series: MetricSeriesConfig[]
  yTicks?: number[]
  yTickStep?: number
  adaptiveMaxTickCount?: number
  adaptiveTickRangeMode?: 'zeroBased' | 'dataFocused'
  yPadding: ChartYAxisPadding
  yTickLabelFormatter: (value: number) => string
}

const CHART_VIEWBOX_WIDTH = 108
const CHART_VIEWBOX_HEIGHT = 48
const CHART_LINE_STROKE_WIDTH = 0.8
const AUDIO_BITRATE_Y_TICKS = [30, 60, 90, 120, 160, 200]
const VIDEO_BITRATE_TICK_STEP_KBPS = 250
const LATENCY_Y_TICK_STEP_MS = 100
const ADAPTIVE_TICK_HEADROOM_RATIO = 0.25
const BITRATE_Y_PADDING: ChartYAxisPadding = { topRatio: 0.14, bottomRatio: 0.1 }
const RENDERING_RATE_Y_PADDING: ChartYAxisPadding = { topRatio: 0.16, bottomRatio: 0.12 }
const LATENCY_Y_PADDING: ChartYAxisPadding = { topRatio: 0.12, bottomRatio: 0.1 }

const STATS_CHART_CONFIGS: StatsChartConfig[] = [
  {
    key: 'video-bitrate',
    title: 'Video Bitrate (kbps)',
    series: [{ label: 'Video', accessor: (sample) => sample.videoBitrateKbps, colorClass: 'text-cyan-300' }],
    yTickStep: VIDEO_BITRATE_TICK_STEP_KBPS,
    yPadding: BITRATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} kbps`
  },
  {
    key: 'audio-bitrate',
    title: 'Audio Bitrate (kbps)',
    series: [{ label: 'Audio', accessor: (sample) => sample.audioBitrateKbps, colorClass: 'text-emerald-300' }],
    yTicks: AUDIO_BITRATE_Y_TICKS,
    yPadding: BITRATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} kbps`
  },
  {
    key: 'screenshare-bitrate',
    title: 'ScreenShare Bitrate (kbps)',
    series: [{ label: 'Screen', accessor: (sample) => sample.screenShareBitrateKbps, colorClass: 'text-orange-300' }],
    yTickStep: VIDEO_BITRATE_TICK_STEP_KBPS,
    yPadding: BITRATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} kbps`
  },
  {
    key: 'local-video-encode',
    title: 'Local Video Encode (Capture→EncodeDone)',
    series: [
      {
        label: 'Camera',
        accessor: (sample) => sample.localCameraSend?.captureToEncodeDoneMs,
        colorClass: 'text-cyan-300'
      },
      {
        label: 'Screen',
        accessor: (sample) => sample.localScreenShareSend?.captureToEncodeDoneMs,
        colorClass: 'text-orange-300'
      }
    ],
    yTickStep: 25,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    key: 'local-video-send-queue-wait',
    title: 'Local Video Send Queue Wait',
    series: [
      {
        label: 'Camera',
        accessor: (sample) => sample.localCameraSend?.queueWaitMs,
        colorClass: 'text-cyan-300'
      },
      {
        label: 'Screen',
        accessor: (sample) => sample.localScreenShareSend?.queueWaitMs,
        colorClass: 'text-orange-300'
      }
    ],
    yTickStep: 25,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    key: 'local-video-send-active',
    title: 'Local Video Send Active',
    series: [
      {
        label: 'Camera total',
        accessor: (sample) => sample.localCameraSend?.sendActiveMs,
        colorClass: 'text-emerald-300'
      },
      {
        label: 'Camera object',
        accessor: (sample) => sample.localCameraSend?.objectSendMs,
        colorClass: 'text-lime-300'
      },
      {
        label: 'Screen total',
        accessor: (sample) => sample.localScreenShareSend?.sendActiveMs,
        colorClass: 'text-amber-300'
      }
    ],
    yTickStep: 25,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    key: 'local-video-send-queue-depth',
    title: 'Local Video Send Queue Depth',
    series: [
      {
        label: 'Camera sendQ',
        accessor: (sample) => sample.localCameraSend?.queueDepth,
        colorClass: 'text-fuchsia-300'
      },
      {
        label: 'Camera encQ',
        accessor: (sample) => sample.localCameraSend?.encodeQueueSize,
        colorClass: 'text-violet-300'
      },
      {
        label: 'Screen sendQ',
        accessor: (sample) => sample.localScreenShareSend?.queueDepth,
        colorClass: 'text-rose-300'
      },
      {
        label: 'Screen encQ',
        accessor: (sample) => sample.localScreenShareSend?.encodeQueueSize,
        colorClass: 'text-red-300'
      }
    ],
    yTickStep: 1,
    adaptiveMaxTickCount: 8,
    yPadding: RENDERING_RATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)}`
  },
  {
    key: 'local-video-send-object-bytes',
    title: 'Local Video Object Size',
    series: [
      {
        label: 'Camera',
        accessor: (sample) => sample.localCameraSend && sample.localCameraSend.objectBytes / 1024,
        colorClass: 'text-sky-300'
      },
      {
        label: 'Screen',
        accessor: (sample) => sample.localScreenShareSend && sample.localScreenShareSend.objectBytes / 1024,
        colorClass: 'text-yellow-300'
      }
    ],
    yTickStep: 25,
    yPadding: BITRATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} KB`
  },
  {
    key: 'playout-buffer',
    title: 'Playout Buffer (ms)',
    series: [
      { label: 'Buffer', accessor: (sample) => sample.bufferMs, colorClass: 'text-emerald-300' },
      { label: 'Target', accessor: (sample) => sample.targetBufferMs, colorClass: 'text-orange-300' },
      { label: 'Output latency', accessor: (sample) => sample.outputLatencyMs, colorClass: 'text-sky-300' },
      { label: 'Jitter (p-p)', accessor: (sample) => sample.arrivalSpreadMs, colorClass: 'text-rose-300' }
    ],
    yTickStep: LATENCY_Y_TICK_STEP_MS,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    key: 'viewer-delay',
    title: 'Delay (capture → display, ms)',
    series: [{ label: 'Delay', accessor: (sample) => sample.viewerDelayMs, colorClass: 'text-cyan-300' }],
    yTickStep: LATENCY_Y_TICK_STEP_MS,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    key: 'av-offset',
    title: 'A/V Offset (video − audio, ms)',
    series: [{ label: 'Offset', accessor: (sample) => sample.syncOffsetMs, colorClass: 'text-violet-300' }],
    yTickStep: 20,
    adaptiveTickRangeMode: 'dataFocused',
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
    chart.series.some((series) => buildMetricEntries(samples, series.accessor).length > 0)
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
      {visibleCharts.map((chart) => (
        <MetricChart
          key={chart.key}
          title={chart.title}
          samples={samples}
          series={chart.series}
          yTicks={
            chart.yTickStep
              ? buildAdaptiveTicks(
                  samples,
                  chart.series.map((series) => series.accessor),
                  chart.yTickStep,
                  chart.adaptiveMaxTickCount,
                  chart.adaptiveTickRangeMode
                )
              : chart.yTicks
          }
          yPadding={chart.yPadding}
          yTickLabelFormatter={chart.yTickLabelFormatter}
          chartHeightClass={chartHeightClass}
        />
      ))}
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
  series,
  yTicks,
  yPadding,
  yTickLabelFormatter,
  chartHeightClass
}: {
  title: string
  samples: SidebarStatsSample[]
  series: MetricSeriesConfig[]
  yTicks?: number[]
  yPadding: ChartYAxisPadding
  yTickLabelFormatter: (value: number) => string
  chartHeightClass: string
}) {
  const [hiddenSeriesLabels, setHiddenSeriesLabels] = useState<Set<string>>(new Set())
  const seriesEntries = series.map((seriesConfig) => ({
    ...seriesConfig,
    entries: buildMetricEntries(samples, seriesConfig.accessor)
  }))
  const visibleSeriesEntries = seriesEntries.filter((seriesConfig) => !hiddenSeriesLabels.has(seriesConfig.label))
  const hasRenderableSeries = visibleSeriesEntries.some((seriesConfig) => seriesConfig.entries.length >= 2)
  const dataValues = visibleSeriesEntries.flatMap((seriesConfig) => seriesConfig.entries.map((entry) => entry.value))
  const tickValuesAll = (yTicks ?? []).filter((value) => Number.isFinite(value))
  const min = Math.min(...dataValues, ...tickValuesAll)
  const max = Math.max(...dataValues, ...tickValuesAll)
  const rawMin = Number.isFinite(min) ? min : 0
  const rawMax = Number.isFinite(max) ? max : rawMin + 1
  const rawRange = Math.max(1, rawMax - rawMin)
  const axisMin = rawMin - rawRange * yPadding.bottomRatio
  const axisMax = Math.max(axisMin + 1, rawMax + rawRange * yPadding.topRatio)
  const tickValues = tickValuesAll.filter((tick) => tick >= axisMin && tick <= axisMax)
  const denominator = Math.max(1, samples.length - 1)
  const toChartX = (entry: MetricChartEntry) => ((entry.index / denominator) * CHART_VIEWBOX_WIDTH).toFixed(2)
  const toChartY = (value: number) => ((axisMax - value) / (axisMax - axisMin)) * CHART_VIEWBOX_HEIGHT
  const toPolylinePoints = (entries: MetricChartEntry[]) =>
    entries.map((entry) => `${toChartX(entry)},${toChartY(entry.value).toFixed(2)}`).join(' ')
  const toAreaPoints = (entries: MetricChartEntry[]) => {
    const baselineY = CHART_VIEWBOX_HEIGHT.toFixed(2)
    return `${toChartX(entries[0])},${baselineY} ${toPolylinePoints(entries)} ${toChartX(entries[entries.length - 1])},${baselineY}`
  }
  const toggleSeries = (label: string) =>
    setHiddenSeriesLabels((prev) => {
      const next = new Set(prev)
      if (next.has(label)) {
        next.delete(label)
      } else {
        next.add(label)
      }
      return next
    })

  return (
    <section className="rounded-lg border border-white/10 bg-white/[0.03] p-3">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h4 className="text-xs font-semibold text-blue-100">{title}</h4>
        <div className="flex flex-wrap items-center gap-2 text-[10px]">
          {seriesEntries.map((seriesConfig) => {
            const hidden = hiddenSeriesLabels.has(seriesConfig.label)
            return (
              <button
                key={`${title}-${seriesConfig.label}`}
                type="button"
                onClick={() => toggleSeries(seriesConfig.label)}
                className={`inline-flex items-center gap-1.5 rounded border px-1.5 py-0.5 text-blue-100/90 transition ${
                  hidden ? 'border-white/10 opacity-40' : 'border-white/20 bg-white/[0.04]'
                }`}
                title={hidden ? 'Show series' : 'Hide series'}
              >
                <span className={`inline-block h-2 w-2 rounded-sm bg-current ${seriesConfig.colorClass}`} />
                <span>{seriesConfig.label}</span>
              </button>
            )
          })}
        </div>
      </div>
      {hasRenderableSeries ? (
        <svg
          viewBox={`0 0 ${CHART_VIEWBOX_WIDTH} ${CHART_VIEWBOX_HEIGHT}`}
          className={`block w-full ${chartHeightClass} overflow-visible rounded bg-black/20`}
        >
          {tickValues.map((tick) => {
            const y = toChartY(tick)
            return (
              <g key={`${title}-tick-${tick}`}>
                <line
                  x1={0}
                  y1={y}
                  x2={CHART_VIEWBOX_WIDTH}
                  y2={y}
                  stroke="rgba(148, 163, 184, 0.28)"
                  strokeDasharray="1.2 1.2"
                />
                <text x={1.2} y={Math.max(2.5, y - 0.6)} fontSize={2.4} fill="rgba(191, 219, 254, 0.75)">
                  {yTickLabelFormatter(tick)}
                </text>
              </g>
            )
          })}
          {visibleSeriesEntries.map((seriesConfig) => {
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
                  strokeWidth={CHART_LINE_STROKE_WIDTH}
                  vectorEffect="non-scaling-stroke"
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

function buildAdaptiveTicks(
  samples: SidebarStatsSample[],
  accessors: MetricAccessor[],
  step: number,
  maxTickCount?: number,
  rangeMode: 'zeroBased' | 'dataFocused' = 'zeroBased'
): number[] {
  const values = accessors.flatMap((accessor) => buildMetricEntries(samples, accessor).map((entry) => entry.value))
  if (!values.length) {
    return [step]
  }
  const minValue = Math.min(...values)
  const maxValue = Math.max(...values)
  const minWithMargin = Math.max(0, minValue - step)
  const maxWithHeadroom = maxValue * (1 + ADAPTIVE_TICK_HEADROOM_RATIO)
  const spanForTicks = rangeMode === 'dataFocused' ? Math.max(step, maxWithHeadroom - minWithMargin) : maxWithHeadroom
  const resolvedStep = resolveAdaptiveStep(step, spanForTicks, maxTickCount)
  const startTick =
    rangeMode === 'dataFocused'
      ? Math.max(resolvedStep, Math.floor(minWithMargin / resolvedStep) * resolvedStep)
      : resolvedStep
  const maxTick = Math.max(startTick, Math.ceil(maxWithHeadroom / resolvedStep) * resolvedStep)
  const ticks: number[] = []
  for (let tick = startTick; tick <= maxTick; tick += resolvedStep) {
    ticks.push(tick)
  }
  return ticks
}

function resolveAdaptiveStep(baseStep: number, maxWithHeadroom: number, maxTickCount?: number): number {
  if (!maxTickCount) {
    return baseStep
  }
  const requiredStep = maxWithHeadroom / maxTickCount
  if (requiredStep <= baseStep) {
    return baseStep
  }
  return Math.ceil(requiredStep / baseStep) * baseStep
}
