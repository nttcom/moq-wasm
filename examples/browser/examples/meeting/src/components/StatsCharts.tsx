import { useState } from 'react'
import type { LiveStats } from '@player/locLive'
import type { RemoteVideoSource } from '../media/mediaSubscriber'
import type { LocalStatsSample, RemoteStatsSample } from '../types/stats'

type ChartYAxisPadding = {
  topRatio: number
  bottomRatio: number
}
type ChartScale = {
  yTicks?: number[]
  yTickStep?: number
  adaptiveMaxTickCount?: number
  adaptiveTickRangeMode?: 'zeroBased' | 'dataFocused'
  yPadding: ChartYAxisPadding
  unit: 'kbps' | 'ms' | 'KB' | ''
}
type ChartPoint = { timestamp: number; value: number }
/// A series keeps the same `key` in every chart of a list, so hiding it hides it everywhere.
type ChartSeries = { key: string; label: string; colorClass: string; points: ChartPoint[] }
type StatsChart = { key: string; title: string; scale: ChartScale; series: ChartSeries[] }
type TimeRange = { start: number; end: number }

type SampleSeriesConfig<S> = {
  label: string
  accessor: (sample: S) => number | null | undefined
  colorClass: string
}
type SampleChartConfig<S> = { key: string; title: string; scale: ChartScale; series: SampleSeriesConfig<S>[] }

const CHART_HEIGHT_CLASS = 'h-64'
const CHART_VIEWBOX_WIDTH = 108
const CHART_VIEWBOX_HEIGHT = 48
const CHART_LINE_STROKE_WIDTH = 0.8
const ADAPTIVE_TICK_HEADROOM_RATIO = 0.25
const BITRATE_Y_PADDING: ChartYAxisPadding = { topRatio: 0.14, bottomRatio: 0.1 }
const RENDERING_RATE_Y_PADDING: ChartYAxisPadding = { topRatio: 0.16, bottomRatio: 0.12 }
const LATENCY_Y_PADDING: ChartYAxisPadding = { topRatio: 0.12, bottomRatio: 0.1 }

const VIDEO_BITRATE_SCALE: ChartScale = { yTickStep: 250, yPadding: BITRATE_Y_PADDING, unit: 'kbps' }
const AUDIO_BITRATE_SCALE: ChartScale = {
  yTicks: [30, 60, 90, 120, 160, 200],
  yPadding: BITRATE_Y_PADDING,
  unit: 'kbps'
}
const SEND_TIME_SCALE: ChartScale = { yTickStep: 25, yPadding: LATENCY_Y_PADDING, unit: 'ms' }
const QUEUE_DEPTH_SCALE: ChartScale = {
  yTickStep: 1,
  adaptiveMaxTickCount: 8,
  yPadding: RENDERING_RATE_Y_PADDING,
  unit: ''
}
const OBJECT_SIZE_SCALE: ChartScale = { yTickStep: 25, yPadding: BITRATE_Y_PADDING, unit: 'KB' }
const PLAYOUT_SCALE: ChartScale = { yTickStep: 100, yPadding: LATENCY_Y_PADDING, unit: 'ms' }
const AV_OFFSET_SCALE: ChartScale = {
  yTickStep: 20,
  adaptiveTickRangeMode: 'dataFocused',
  yPadding: LATENCY_Y_PADDING,
  unit: 'ms'
}

const SERIES_COLOR_CLASSES = [
  'text-cyan-300',
  'text-orange-300',
  'text-emerald-300',
  'text-fuchsia-300',
  'text-yellow-300',
  'text-sky-300',
  'text-rose-300',
  'text-lime-300',
  'text-violet-300',
  'text-amber-300'
]

const SEND_CHART_CONFIGS: SampleChartConfig<LocalStatsSample>[] = [
  {
    key: 'video-bitrate',
    title: 'Video Bitrate (kbps)',
    scale: VIDEO_BITRATE_SCALE,
    series: [
      { label: 'Camera', accessor: (sample) => sample.cameraBitrateKbps, colorClass: 'text-cyan-300' },
      { label: 'Screen', accessor: (sample) => sample.screenShareBitrateKbps, colorClass: 'text-orange-300' }
    ]
  },
  {
    key: 'audio-bitrate',
    title: 'Audio Bitrate (kbps)',
    scale: AUDIO_BITRATE_SCALE,
    series: [{ label: 'Audio', accessor: (sample) => sample.audioBitrateKbps, colorClass: 'text-emerald-300' }]
  },
  {
    key: 'video-encode',
    title: 'Video Encode (Capture→EncodeDone)',
    scale: SEND_TIME_SCALE,
    series: [
      { label: 'Camera', accessor: (sample) => sample.cameraSend?.captureToEncodeDoneMs, colorClass: 'text-cyan-300' },
      {
        label: 'Screen',
        accessor: (sample) => sample.screenShareSend?.captureToEncodeDoneMs,
        colorClass: 'text-orange-300'
      }
    ]
  },
  {
    key: 'video-send-queue-wait',
    title: 'Video Send Queue Wait',
    scale: SEND_TIME_SCALE,
    series: [
      { label: 'Camera', accessor: (sample) => sample.cameraSend?.queueWaitMs, colorClass: 'text-cyan-300' },
      { label: 'Screen', accessor: (sample) => sample.screenShareSend?.queueWaitMs, colorClass: 'text-orange-300' }
    ]
  },
  {
    key: 'video-send-active',
    title: 'Video Send Active',
    scale: SEND_TIME_SCALE,
    series: [
      { label: 'Camera total', accessor: (sample) => sample.cameraSend?.sendActiveMs, colorClass: 'text-emerald-300' },
      { label: 'Camera object', accessor: (sample) => sample.cameraSend?.objectSendMs, colorClass: 'text-lime-300' },
      {
        label: 'Screen total',
        accessor: (sample) => sample.screenShareSend?.sendActiveMs,
        colorClass: 'text-amber-300'
      }
    ]
  },
  {
    key: 'video-send-queue-depth',
    title: 'Video Send Queue Depth',
    scale: QUEUE_DEPTH_SCALE,
    series: [
      { label: 'Camera sendQ', accessor: (sample) => sample.cameraSend?.queueDepth, colorClass: 'text-fuchsia-300' },
      { label: 'Camera encQ', accessor: (sample) => sample.cameraSend?.encodeQueueSize, colorClass: 'text-violet-300' },
      { label: 'Screen sendQ', accessor: (sample) => sample.screenShareSend?.queueDepth, colorClass: 'text-rose-300' },
      {
        label: 'Screen encQ',
        accessor: (sample) => sample.screenShareSend?.encodeQueueSize,
        colorClass: 'text-red-300'
      }
    ]
  },
  {
    key: 'video-send-object-bytes',
    title: 'Video Object Size',
    scale: OBJECT_SIZE_SCALE,
    series: [
      {
        label: 'Camera',
        accessor: (sample) => sample.cameraSend && sample.cameraSend.objectBytes / 1024,
        colorClass: 'text-cyan-300'
      },
      {
        label: 'Screen',
        accessor: (sample) => sample.screenShareSend && sample.screenShareSend.objectBytes / 1024,
        colorClass: 'text-orange-300'
      }
    ]
  }
]

const primaryStats = (sample: RemoteStatsSample) => sample.camera ?? sample.screenshare

const REMOTE_MEMBER_CHART_CONFIGS: SampleChartConfig<RemoteStatsSample>[] = [
  {
    key: 'video-bitrate',
    title: 'Video Bitrate (kbps)',
    scale: VIDEO_BITRATE_SCALE,
    series: [
      { label: 'Camera', accessor: (sample) => sample.camera?.receivedKbps, colorClass: 'text-cyan-300' },
      { label: 'Screen', accessor: (sample) => sample.screenshare?.receivedKbps, colorClass: 'text-orange-300' }
    ]
  },
  {
    key: 'audio-bitrate',
    title: 'Audio Bitrate (kbps)',
    scale: AUDIO_BITRATE_SCALE,
    series: [{ label: 'Audio', accessor: (sample) => sample.camera?.receivedAudioKbps, colorClass: 'text-emerald-300' }]
  },
  {
    key: 'playout-buffer',
    title: 'Playout Buffer (ms)',
    scale: PLAYOUT_SCALE,
    series: [
      { label: 'Buffer', accessor: (sample) => primaryStats(sample)?.bufferMs, colorClass: 'text-emerald-300' },
      { label: 'Target', accessor: (sample) => primaryStats(sample)?.targetBufferMs, colorClass: 'text-orange-300' },
      {
        label: 'Output latency',
        accessor: (sample) => primaryStats(sample)?.outputLatencyMs,
        colorClass: 'text-sky-300'
      },
      {
        label: 'Jitter (p-p)',
        accessor: (sample) => primaryStats(sample)?.arrivalSpreadMs,
        colorClass: 'text-rose-300'
      }
    ]
  },
  {
    key: 'viewer-delay',
    title: 'Pub to sub delay (ms)',
    scale: PLAYOUT_SCALE,
    series: [
      {
        label: 'Pub to sub delay',
        accessor: (sample) => primaryStats(sample)?.viewerDelayMs,
        colorClass: 'text-cyan-300'
      }
    ]
  },
  {
    key: 'av-offset',
    title: 'A/V Offset (video − audio, ms)',
    scale: AV_OFFSET_SCALE,
    series: [
      { label: 'Offset', accessor: (sample) => primaryStats(sample)?.syncOffsetMs, colorClass: 'text-violet-300' }
    ]
  }
]

type ReceiveMetric = {
  key: string
  title: string
  scale: ChartScale
  value: (stats: LiveStats) => number | undefined
  cameraOnly?: true
}

const BOTH_SOURCES: RemoteVideoSource[] = ['camera', 'screenshare']

const RECEIVE_METRICS: ReceiveMetric[] = [
  {
    key: 'video-bitrate',
    title: 'Video Bitrate (kbps)',
    scale: VIDEO_BITRATE_SCALE,
    value: (stats) => stats.receivedKbps
  },
  {
    key: 'audio-bitrate',
    title: 'Audio Bitrate (kbps)',
    scale: AUDIO_BITRATE_SCALE,
    value: (stats) => stats.receivedAudioKbps,
    cameraOnly: true
  },
  {
    key: 'playout-buffer',
    title: 'Playout Buffer (ms)',
    scale: PLAYOUT_SCALE,
    value: (stats) => stats.bufferMs
  },
  {
    key: 'target-buffer',
    title: 'Target Buffer (ms)',
    scale: PLAYOUT_SCALE,
    value: (stats) => stats.targetBufferMs
  },
  {
    key: 'output-latency',
    title: 'Output Latency (ms)',
    scale: PLAYOUT_SCALE,
    value: (stats) => stats.outputLatencyMs
  },
  {
    key: 'jitter',
    title: 'Jitter (p-p, ms)',
    scale: PLAYOUT_SCALE,
    value: (stats) => stats.arrivalSpreadMs
  },
  {
    key: 'viewer-delay',
    title: 'Pub to sub delay (ms)',
    scale: PLAYOUT_SCALE,
    value: (stats) => stats.viewerDelayMs
  },
  {
    key: 'av-offset',
    title: 'A/V Offset (video − audio, ms)',
    scale: AV_OFFSET_SCALE,
    value: (stats) => stats.syncOffsetMs
  }
]

export type RemoteMemberStats = { id: string; name: string; samples: RemoteStatsSample[] }

type StatsTab = 'send' | 'receive'

export function MeetingStats({
  localSamples,
  remoteMembers
}: {
  localSamples: LocalStatsSample[]
  remoteMembers: RemoteMemberStats[]
}) {
  const [tab, setTab] = useState<StatsTab>('send')
  return (
    <div className="space-y-4">
      <div className="inline-flex rounded-md border border-white/10 bg-white/5 p-0.5 text-sm" role="tablist">
        {(['send', 'receive'] as const).map((value) => (
          <button
            key={value}
            type="button"
            role="tab"
            aria-selected={tab === value}
            data-testid={`stats-tab-${value}`}
            onClick={() => setTab(value)}
            className={`rounded px-4 py-1 transition ${
              tab === value ? 'bg-white/15 text-white' : 'text-blue-200 hover:text-white'
            }`}
          >
            {value === 'send' ? 'Send' : 'Receive'}
          </button>
        ))}
      </div>
      {tab === 'send' ? (
        <SendStatsCharts samples={localSamples} />
      ) : (
        <StatsChartList charts={buildReceiveCharts(remoteMembers)} emptyMessage="No received media yet." />
      )}
    </div>
  )
}

export function SendStatsCharts({ samples }: { samples: LocalStatsSample[] }) {
  return <StatsChartList charts={buildSampleCharts(SEND_CHART_CONFIGS, samples)} emptyMessage="No sent media yet." />
}

export function RemoteMemberStatsCharts({ samples }: { samples: RemoteStatsSample[] }) {
  return (
    <StatsChartList
      charts={buildSampleCharts(REMOTE_MEMBER_CHART_CONFIGS, samples)}
      emptyMessage="No subscribed track metrics for this member."
    />
  )
}

function buildSampleCharts<S extends { timestamp: number }>(
  configs: SampleChartConfig<S>[],
  samples: S[]
): StatsChart[] {
  return configs.map((config) => ({
    key: config.key,
    title: config.title,
    scale: config.scale,
    series: config.series.map((series) => ({
      key: series.label,
      label: series.label,
      colorClass: series.colorClass,
      points: toPoints(samples, series.accessor)
    }))
  }))
}

/// Each video keeps the same color in every chart.
function buildReceiveCharts(members: RemoteMemberStats[]): StatsChart[] {
  const videos = members.flatMap((member) =>
    BOTH_SOURCES.filter((source) => member.samples.some((sample) => sample[source])).map((source) => ({
      key: `${member.id}:${source}`,
      label: source === 'camera' ? member.name : `${member.name} · Screen`,
      member,
      source
    }))
  )
  return RECEIVE_METRICS.map((metric) => ({
    key: metric.key,
    title: metric.title,
    scale: metric.scale,
    series: videos.flatMap((video, index) =>
      !metric.cameraOnly || video.source === 'camera'
        ? [
            {
              key: video.key,
              label: video.label,
              colorClass: SERIES_COLOR_CLASSES[index % SERIES_COLOR_CLASSES.length],
              points: toPoints(video.member.samples, (sample) => {
                const stats = sample[video.source]
                return stats && metric.value(stats)
              })
            }
          ]
        : []
    )
  }))
}

function toPoints<S extends { timestamp: number }>(
  samples: S[],
  accessor: (sample: S) => number | null | undefined
): ChartPoint[] {
  return samples.flatMap((sample) => {
    const value = accessor(sample)
    return typeof value === 'number' && Number.isFinite(value) ? [{ timestamp: sample.timestamp, value }] : []
  })
}

function StatsChartList({ charts, emptyMessage }: { charts: StatsChart[]; emptyMessage: string }) {
  const [hiddenSeriesKeys, setHiddenSeriesKeys] = useState<Set<string>>(new Set())
  const visibleCharts = charts
    .map((chart) => ({ ...chart, series: chart.series.filter((series) => series.points.length > 0) }))
    .filter((chart) => chart.series.length > 0)

  if (!visibleCharts.length) {
    return (
      <div className="rounded-md border border-white/10 bg-white/[0.03] p-4 text-xs text-blue-200">{emptyMessage}</div>
    )
  }

  const timestamps = visibleCharts.flatMap((chart) =>
    chart.series.flatMap((series) => series.points.map((point) => point.timestamp))
  )
  const timeRange = { start: Math.min(...timestamps), end: Math.max(...timestamps) }
  const toggleSeries = (key: string) =>
    setHiddenSeriesKeys((prev) => {
      const next = new Set(prev)
      if (next.has(key)) {
        next.delete(key)
      } else {
        next.add(key)
      }
      return next
    })

  return (
    <div className="space-y-4">
      {visibleCharts.map((chart) => (
        <MetricChart
          key={chart.key}
          chart={chart}
          timeRange={timeRange}
          hiddenSeriesKeys={hiddenSeriesKeys}
          onToggleSeries={toggleSeries}
        />
      ))}
    </div>
  )
}

function MetricChart({
  chart,
  timeRange,
  hiddenSeriesKeys,
  onToggleSeries
}: {
  chart: StatsChart
  timeRange: TimeRange
  hiddenSeriesKeys: Set<string>
  onToggleSeries: (key: string) => void
}) {
  const { title, scale, series } = chart
  const yTicks = scale.yTickStep
    ? buildAdaptiveTicks(
        series.flatMap((entry) => entry.points.map((point) => point.value)),
        scale.yTickStep,
        scale.adaptiveMaxTickCount,
        scale.adaptiveTickRangeMode
      )
    : scale.yTicks
  const visibleSeries = series.filter((entry) => !hiddenSeriesKeys.has(entry.key))
  const hasRenderableSeries = visibleSeries.some((entry) => entry.points.length >= 2)
  const dataValues = visibleSeries.flatMap((entry) => entry.points.map((point) => point.value))
  const tickValuesAll = (yTicks ?? []).filter((value) => Number.isFinite(value))
  const min = Math.min(...dataValues, ...tickValuesAll)
  const max = Math.max(...dataValues, ...tickValuesAll)
  const rawMin = Number.isFinite(min) ? min : 0
  const rawMax = Number.isFinite(max) ? max : rawMin + 1
  const rawRange = Math.max(1, rawMax - rawMin)
  const axisMin = rawMin - rawRange * scale.yPadding.bottomRatio
  const axisMax = Math.max(axisMin + 1, rawMax + rawRange * scale.yPadding.topRatio)
  const tickValues = tickValuesAll.filter((tick) => tick >= axisMin && tick <= axisMax)
  const duration = Math.max(1, timeRange.end - timeRange.start)
  const toChartX = (point: ChartPoint) =>
    (((point.timestamp - timeRange.start) / duration) * CHART_VIEWBOX_WIDTH).toFixed(2)
  const toChartY = (value: number) => ((axisMax - value) / (axisMax - axisMin)) * CHART_VIEWBOX_HEIGHT
  const toPolylinePoints = (points: ChartPoint[]) =>
    points.map((point) => `${toChartX(point)},${toChartY(point.value).toFixed(2)}`).join(' ')
  const toAreaPoints = (points: ChartPoint[]) => {
    const baselineY = CHART_VIEWBOX_HEIGHT.toFixed(2)
    return `${toChartX(points[0])},${baselineY} ${toPolylinePoints(points)} ${toChartX(points[points.length - 1])},${baselineY}`
  }

  return (
    <section className="rounded-lg border border-white/10 bg-white/[0.03] p-3">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h4 className="text-xs font-semibold text-blue-100">{title}</h4>
        <div className="flex flex-wrap items-center gap-2 text-[10px]">
          {series.map((entry) => {
            const hidden = hiddenSeriesKeys.has(entry.key)
            return (
              <button
                key={entry.key}
                type="button"
                onClick={() => onToggleSeries(entry.key)}
                className={`inline-flex items-center gap-1.5 rounded border px-1.5 py-0.5 text-blue-100/90 transition ${
                  hidden ? 'border-white/10 opacity-40' : 'border-white/20 bg-white/[0.04]'
                }`}
                title={hidden ? 'Show series' : 'Hide series'}
              >
                <span className={`inline-block h-2 w-2 rounded-sm bg-current ${entry.colorClass}`} />
                <span>{entry.label}</span>
              </button>
            )
          })}
        </div>
      </div>
      {hasRenderableSeries ? (
        <svg
          viewBox={`0 0 ${CHART_VIEWBOX_WIDTH} ${CHART_VIEWBOX_HEIGHT}`}
          className={`block w-full ${CHART_HEIGHT_CLASS} overflow-visible rounded bg-black/20`}
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
                  {formatTickLabel(tick, scale.unit)}
                </text>
              </g>
            )
          })}
          {visibleSeries.map((entry) => {
            if (entry.points.length < 2) {
              return null
            }
            return (
              <g key={entry.key} className={entry.colorClass}>
                <polygon points={toAreaPoints(entry.points)} fill="currentColor" fillOpacity={0.16} />
                <polyline
                  points={toPolylinePoints(entry.points)}
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
          className={`flex ${CHART_HEIGHT_CLASS} items-center justify-center rounded bg-black/20 text-xs text-blue-200`}
        >
          No data
        </div>
      )}
    </section>
  )
}

function formatTickLabel(value: number, unit: ChartScale['unit']): string {
  return unit ? `${Math.round(value)} ${unit}` : `${Math.round(value)}`
}

function buildAdaptiveTicks(
  values: number[],
  step: number,
  maxTickCount?: number,
  rangeMode: 'zeroBased' | 'dataFocused' = 'zeroBased'
): number[] {
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
