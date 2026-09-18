import { useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
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
  adaptiveTickRangeMode?: 'zeroBased' | 'dataFocused'
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

const CHART_VIEWBOX_WIDTH = 108
const CHART_VIEWBOX_HEIGHT = 48
const SINGLE_CHART_LINE_STROKE_WIDTH = 0.9
const MULTI_CHART_LINE_STROKE_WIDTH = 0.8
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
const CHART_DRAG_X_ZOOM_MIN = 1
const CHART_DRAG_Y_ZOOM_MIN = 0.5
const CHART_DRAG_ZOOM_MAX_X = 6
const CHART_DRAG_ZOOM_MAX_Y = 8
const CHART_DRAG_ZOOM_SENSITIVITY_PX = 240

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
    kind: 'multi',
    key: 'local-video-encode',
    title: 'Local Video Encode (Capture→EncodeDone)',
    series: [
      {
        label: 'Camera',
        accessor: (sample) => sample.localCameraCaptureToEncodeDoneMs,
        colorClass: 'text-cyan-300'
      },
      {
        label: 'Screen',
        accessor: (sample) => sample.localScreenShareCaptureToEncodeDoneMs,
        colorClass: 'text-orange-300'
      }
    ],
    yTickStep: 25,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    kind: 'multi',
    key: 'local-video-send-queue-wait',
    title: 'Local Video Send Queue Wait',
    series: [
      {
        label: 'Camera',
        accessor: (sample) => sample.localCameraSendQueueWaitMs,
        colorClass: 'text-cyan-300'
      },
      {
        label: 'Screen',
        accessor: (sample) => sample.localScreenShareSendQueueWaitMs,
        colorClass: 'text-orange-300'
      }
    ],
    yTickStep: 25,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    kind: 'multi',
    key: 'local-video-send-active',
    title: 'Local Video Send Active',
    series: [
      {
        label: 'Camera total',
        accessor: (sample) => sample.localCameraSendActiveMs,
        colorClass: 'text-emerald-300'
      },
      {
        label: 'Camera object',
        accessor: (sample) => sample.localCameraSendObjectMs,
        colorClass: 'text-lime-300'
      },
      {
        label: 'Camera serialize',
        accessor: (sample) => sample.localCameraSendSerializeMs,
        colorClass: 'text-teal-300'
      },
      {
        label: 'Screen total',
        accessor: (sample) => sample.localScreenShareSendActiveMs,
        colorClass: 'text-amber-300'
      }
    ],
    yTickStep: 25,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    kind: 'multi',
    key: 'local-video-send-queue-depth',
    title: 'Local Video Send Queue Depth',
    series: [
      {
        label: 'Camera sendQ',
        accessor: (sample) => sample.localCameraSendQueueDepth,
        colorClass: 'text-fuchsia-300'
      },
      {
        label: 'Camera encQ',
        accessor: (sample) => sample.localCameraEncodeQueueSize,
        colorClass: 'text-violet-300'
      },
      {
        label: 'Screen sendQ',
        accessor: (sample) => sample.localScreenShareSendQueueDepth,
        colorClass: 'text-rose-300'
      },
      {
        label: 'Screen encQ',
        accessor: (sample) => sample.localScreenShareEncodeQueueSize,
        colorClass: 'text-red-300'
      }
    ],
    yTickStep: 1,
    adaptiveMaxTickCount: 8,
    yPadding: RENDERING_RATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)}`
  },
  {
    kind: 'multi',
    key: 'local-video-send-object-bytes',
    title: 'Local Video Object Size',
    series: [
      {
        label: 'Camera',
        accessor: (sample) =>
          typeof sample.localCameraSendObjectBytes === 'number' && Number.isFinite(sample.localCameraSendObjectBytes)
            ? sample.localCameraSendObjectBytes / 1024
            : null,
        colorClass: 'text-sky-300'
      },
      {
        label: 'Screen',
        accessor: (sample) =>
          typeof sample.localScreenShareSendObjectBytes === 'number' &&
          Number.isFinite(sample.localScreenShareSendObjectBytes)
            ? sample.localScreenShareSendObjectBytes / 1024
            : null,
        colorClass: 'text-yellow-300'
      }
    ],
    yTickStep: 25,
    yPadding: BITRATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} KB`
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
    title: 'Video Latency (Cumulative)',
    series: [
      {
        label: 'Network',
        accessor: (sample) => sample.videoReceiveLatencyMs,
        colorClass: 'text-amber-300'
      },
      {
        label: 'Network+Decode',
        accessor: (sample) =>
          typeof sample.videoReceiveLatencyMs === 'number' &&
          Number.isFinite(sample.videoReceiveLatencyMs) &&
          typeof sample.videoReceiveToDecodeMs === 'number' &&
          Number.isFinite(sample.videoReceiveToDecodeMs)
            ? sample.videoReceiveLatencyMs + sample.videoReceiveToDecodeMs
            : null,
        colorClass: 'text-lime-300'
      },
      {
        label: 'E2E (Network+Render)',
        accessor: (sample) =>
          typeof sample.videoReceiveLatencyMs === 'number' &&
          Number.isFinite(sample.videoReceiveLatencyMs) &&
          typeof sample.videoReceiveToRenderMs === 'number' &&
          Number.isFinite(sample.videoReceiveToRenderMs)
            ? sample.videoReceiveLatencyMs + sample.videoReceiveToRenderMs
            : sample.videoRenderLatencyMs,
        colorClass: 'text-rose-300'
      }
    ],
    yTickStep: LATENCY_Y_TICK_STEP_MS,
    adaptiveTickRangeMode: 'dataFocused',
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
  },
  {
    kind: 'multi',
    key: 'screenshare-pipeline-latency',
    title: 'ScreenShare Pipeline Latency',
    series: [
      {
        label: 'Receive→Decode',
        accessor: (sample) => sample.screenShareReceiveToDecodeMs,
        colorClass: 'text-yellow-300'
      },
      {
        label: 'Receive→Render',
        accessor: (sample) => sample.screenShareReceiveToRenderMs,
        colorClass: 'text-orange-300'
      }
    ],
    yTickStep: LATENCY_Y_TICK_STEP_MS,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    kind: 'multi',
    key: 'video-pacing',
    title: 'Video Pacing',
    series: [
      {
        label: 'Eff interval',
        accessor: (sample) => sample.videoPacingEffectiveIntervalMs,
        colorClass: 'text-violet-300'
      }
    ],
    yTickStep: 25,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    kind: 'multi',
    key: 'video-pacing-buffer',
    title: 'Video Pacing Buffer',
    series: [
      {
        label: 'Buffered',
        accessor: (sample) => sample.videoPacingBufferedFrames,
        colorClass: 'text-emerald-300'
      },
      {
        label: 'Decode queue',
        accessor: (sample) => sample.videoDecodeQueueSize,
        colorClass: 'text-violet-300'
      },
      {
        label: 'Target',
        accessor: (sample) => sample.videoPacingTargetFrames,
        colorClass: 'text-sky-300'
      }
    ],
    yTickStep: 1,
    adaptiveMaxTickCount: 8,
    yPadding: RENDERING_RATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} frames`
  },
  {
    kind: 'multi',
    key: 'screenshare-pacing',
    title: 'ScreenShare Pacing',
    series: [
      {
        label: 'Eff interval',
        accessor: (sample) => sample.screenSharePacingEffectiveIntervalMs,
        colorClass: 'text-rose-300'
      }
    ],
    yTickStep: 25,
    yPadding: LATENCY_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} ms`
  },
  {
    kind: 'multi',
    key: 'screenshare-pacing-buffer',
    title: 'ScreenShare Pacing Buffer',
    series: [
      {
        label: 'Buffered',
        accessor: (sample) => sample.screenSharePacingBufferedFrames,
        colorClass: 'text-yellow-300'
      },
      {
        label: 'Decode queue',
        accessor: (sample) => sample.screenShareDecodeQueueSize,
        colorClass: 'text-rose-300'
      },
      {
        label: 'Target',
        accessor: (sample) => sample.screenSharePacingTargetFrames,
        colorClass: 'text-orange-300'
      }
    ],
    yTickStep: 1,
    adaptiveMaxTickCount: 8,
    yPadding: RENDERING_RATE_Y_PADDING,
    yTickLabelFormatter: (value) => `${Math.round(value)} frames`
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
              chart.adaptiveMaxTickCount,
              chart.adaptiveTickRangeMode
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

type ChartAxisZoomState = {
  xZoom: number
  yZoom: number
}

function clampChartZoom(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return 1
  }
  return Math.min(max, Math.max(min, value))
}

function useChartDragZoom() {
  const [zoom, setZoom] = useState<ChartAxisZoomState>({ xZoom: 1, yZoom: 1 })
  const dragCleanupRef = useRef<(() => void) | null>(null)
  const [isDragging, setIsDragging] = useState(false)

  const onPointerDown = (event: ReactPointerEvent<HTMLElement>) => {
    if (event.button !== 0) {
      return
    }
    event.preventDefault()
    dragCleanupRef.current?.()
    const startX = event.clientX
    const startY = event.clientY
    const startZoom = zoom
    setIsDragging(true)

    const handleMove = (moveEvent: PointerEvent) => {
      const dx = moveEvent.clientX - startX
      const dy = moveEvent.clientY - startY
      setZoom({
        xZoom: clampChartZoom(
          startZoom.xZoom + dx / CHART_DRAG_ZOOM_SENSITIVITY_PX,
          CHART_DRAG_X_ZOOM_MIN,
          CHART_DRAG_ZOOM_MAX_X
        ),
        yZoom: clampChartZoom(
          startZoom.yZoom - dy / CHART_DRAG_ZOOM_SENSITIVITY_PX,
          CHART_DRAG_Y_ZOOM_MIN,
          CHART_DRAG_ZOOM_MAX_Y
        )
      })
    }

    const cleanup = () => {
      window.removeEventListener('pointermove', handleMove)
      window.removeEventListener('pointerup', handleUp)
      window.removeEventListener('pointercancel', handleUp)
      dragCleanupRef.current = null
      setIsDragging(false)
    }

    const handleUp = () => {
      cleanup()
    }

    window.addEventListener('pointermove', handleMove)
    window.addEventListener('pointerup', handleUp)
    window.addEventListener('pointercancel', handleUp)
    dragCleanupRef.current = cleanup
  }

  const resetZoom = () => setZoom({ xZoom: 1, yZoom: 1 })

  return { zoom, isDragging, onPointerDown, resetZoom }
}

function resolveVisibleSampleWindow(totalSamples: number, xZoom: number): { startIndex: number; count: number } {
  if (!Number.isFinite(totalSamples) || totalSamples <= 0) {
    return { startIndex: 0, count: 0 }
  }
  const zoom = Number.isFinite(xZoom) ? Math.max(1, xZoom) : 1
  const count = Math.min(totalSamples, Math.max(2, Math.ceil(totalSamples / zoom)))
  return { startIndex: Math.max(0, totalSamples - count), count }
}

function applyYAxisZoom(axisMin: number, axisMax: number, yZoom: number): { axisMin: number; axisMax: number } {
  const baseMin = Number.isFinite(axisMin) ? axisMin : 0
  const baseMax = Number.isFinite(axisMax) ? axisMax : baseMin + 1
  const baseRange = Math.max(1, baseMax - baseMin)
  const zoom = Number.isFinite(yZoom) ? Math.max(CHART_DRAG_Y_ZOOM_MIN, yZoom) : 1
  const zoomedRange = Math.max(1, baseRange / zoom)
  const center = (baseMin + baseMax) / 2
  return {
    axisMin: center - zoomedRange / 2,
    axisMax: center + zoomedRange / 2
  }
}

function resolveMinorYAxisTicks(tickValues: number[], axisMin: number, axisMax: number, yZoom: number): number[] {
  if (!Number.isFinite(yZoom) || yZoom <= 1.15) {
    return []
  }
  const sortedMajorTicks = Array.from(
    new Set(tickValues.filter((value) => Number.isFinite(value)).map((value) => Number(value.toFixed(6))))
  ).sort((a, b) => a - b)
  const positiveDiffs: number[] = []
  for (let index = 1; index < sortedMajorTicks.length; index += 1) {
    const diff = sortedMajorTicks[index] - sortedMajorTicks[index - 1]
    if (diff > 0) {
      positiveDiffs.push(diff)
    }
  }
  const baseStep = positiveDiffs.length ? Math.min(...positiveDiffs) : NaN
  if (!Number.isFinite(baseStep) || baseStep <= 0) {
    return []
  }

  const subdivisions = yZoom >= 6 ? 5 : yZoom >= 4 ? 4 : yZoom >= 2.5 ? 3 : 2
  let minorStep = baseStep / subdivisions
  if (!Number.isFinite(minorStep) || minorStep <= 0) {
    return []
  }
  const visibleRange = Math.max(1e-6, axisMax - axisMin)
  const maxMinorTickCount = 36
  while (visibleRange / minorStep > maxMinorTickCount && minorStep < baseStep) {
    minorStep *= 2
  }

  const tolerance = Math.max(1e-6, minorStep * 0.08)
  const startTick = Math.ceil(axisMin / minorStep) * minorStep
  const endTick = Math.floor(axisMax / minorStep) * minorStep
  const minorTicks: number[] = []
  for (let tick = startTick; tick <= endTick + tolerance; tick += minorStep) {
    const roundedTick = Number(tick.toFixed(6))
    const isMajor = sortedMajorTicks.some((majorTick) => Math.abs(majorTick - roundedTick) <= tolerance)
    if (!isMajor && roundedTick >= axisMin - tolerance && roundedTick <= axisMax + tolerance) {
      minorTicks.push(roundedTick)
    }
  }
  return minorTicks
}

function resolveMinorYAxisLabelTicks(
  majorTickValues: number[],
  minorTickValues: number[],
  axisMin: number,
  axisMax: number,
  yZoom: number
): number[] {
  if (!minorTickValues.length || !Number.isFinite(yZoom) || yZoom <= 1.35) {
    return []
  }
  const axisRange = Math.max(1e-6, axisMax - axisMin)
  const minLabelGapInViewboxUnits = yZoom >= 4 ? 3.2 : 4
  const minLabelGapValue = (axisRange / CHART_VIEWBOX_HEIGHT) * minLabelGapInViewboxUnits
  const occupied = [...majorTickValues]
  const accepted: number[] = []
  const sortedMinorTicks = [...minorTickValues].sort((a, b) => a - b)
  for (const tick of sortedMinorTicks) {
    if (occupied.some((existing) => Math.abs(existing - tick) < minLabelGapValue)) {
      continue
    }
    accepted.push(tick)
    occupied.push(tick)
  }
  const maxMinorLabelCount = 12
  if (accepted.length <= maxMinorLabelCount) {
    return accepted
  }
  const stride = Math.ceil(accepted.length / maxMinorLabelCount)
  return accepted.filter((_, index) => index % stride === 0)
}

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
  const { zoom, isDragging, onPointerDown, resetZoom } = useChartDragZoom()
  const entries = buildMetricEntries(samples, accessor)
  const visibleWindow = resolveVisibleSampleWindow(samples.length, zoom.xZoom)
  const visibleEntries = entries.filter((entry) => entry.index >= visibleWindow.startIndex)

  const dataValues = visibleEntries.map((entry) => entry.value)
  const tickValuesAll = (yTicks ?? []).filter((value) => Number.isFinite(value))
  const min = dataValues.length
    ? Math.min(...dataValues, ...tickValuesAll)
    : tickValuesAll.length
      ? Math.min(...tickValuesAll)
      : 0
  const max = dataValues.length
    ? Math.max(...dataValues, ...tickValuesAll)
    : tickValuesAll.length
      ? Math.max(...tickValuesAll)
      : 0
  const topPaddingRatio = sanitizePaddingRatio(yPadding?.topRatio)
  const bottomPaddingRatio = sanitizePaddingRatio(yPadding?.bottomRatio)
  const rawMin = Number.isFinite(min) ? min : 0
  const rawMax = Number.isFinite(max) ? max : rawMin + 1
  const rawRange = Math.max(1, rawMax - rawMin)
  const baseAxisMin = rawMin - rawRange * bottomPaddingRatio
  const baseAxisMax = Math.max(baseAxisMin + 1, rawMax + rawRange * topPaddingRatio)
  const { axisMin, axisMax } = applyYAxisZoom(baseAxisMin, baseAxisMax, zoom.yZoom)
  const tickValues = tickValuesAll.filter((tick) => tick >= axisMin && tick <= axisMax)
  const minorTickValues = resolveMinorYAxisTicks(tickValuesAll, axisMin, axisMax, zoom.yZoom)
  const minorLabelTickValues = resolveMinorYAxisLabelTicks(tickValues, minorTickValues, axisMin, axisMax, zoom.yZoom)
  const denominator = Math.max(1, visibleWindow.count - 1)
  const toChartY = (value: number) => ((axisMax - value) / (axisMax - axisMin)) * CHART_VIEWBOX_HEIGHT
  const polyline = visibleEntries
    .map((entry) => {
      const x = ((entry.index - visibleWindow.startIndex) / denominator) * CHART_VIEWBOX_WIDTH
      const y = toChartY(entry.value)
      return `${x.toFixed(2)},${y.toFixed(2)}`
    })
    .join(' ')
  return (
    <section className="rounded-lg border border-white/10 bg-white/[0.03] p-3">
      <div className="mb-2 flex items-center justify-between gap-2">
        <h4 className={`text-xs font-semibold ${colorClass}`}>{title}</h4>
        <div className="flex items-center gap-2 text-[10px] text-blue-200/80">
          {(zoom.xZoom !== 1 || zoom.yZoom !== 1) && (
            <button
              type="button"
              onClick={resetZoom}
              className="rounded border border-white/15 bg-white/5 px-1.5 py-0.5 text-blue-100 hover:bg-white/10"
            >
              Reset
            </button>
          )}
        </div>
      </div>
      {visibleEntries.length >= 2 ? (
        <div
          className={`overflow-x-auto select-none ${isDragging ? 'cursor-grabbing' : 'cursor-grab'}`}
          onPointerDown={onPointerDown}
          title="Drag up/right to zoom in, down/left to zoom out"
        >
          <svg
            viewBox={`0 0 ${CHART_VIEWBOX_WIDTH} ${CHART_VIEWBOX_HEIGHT}`}
            className={`block w-full ${chartHeightClass} overflow-visible rounded bg-black/20`}
          >
            {minorTickValues.map((tick) => {
              const y = toChartY(tick)
              return (
                <line
                  key={`${title}-minor-tick-${tick}`}
                  x1={0}
                  y1={y}
                  x2={CHART_VIEWBOX_WIDTH}
                  y2={y}
                  stroke="rgba(148, 163, 184, 0.14)"
                  strokeDasharray="0.8 1.6"
                />
              )
            })}
            {minorLabelTickValues.map((tick) => {
              const y = toChartY(tick)
              return (
                <text
                  key={`${title}-minor-label-${tick}`}
                  x={1.2}
                  y={Math.max(2.3, y - 0.45)}
                  fontSize={2.05}
                  fill="rgba(191, 219, 254, 0.52)"
                >
                  {yTickLabelFormatter ? yTickLabelFormatter(tick) : formatMetricValue(tick, 0)}
                </text>
              )
            })}
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
              vectorEffect="non-scaling-stroke"
              className={colorClass}
            />
          </svg>
        </div>
      ) : (
        <div
          className={`overflow-x-auto select-none ${isDragging ? 'cursor-grabbing' : 'cursor-grab'}`}
          onPointerDown={onPointerDown}
          title="Drag up/right to zoom in, down/left to zoom out"
        >
          <div
            className={`flex ${chartHeightClass} items-center justify-center rounded bg-black/20 text-xs text-blue-200`}
          >
            No data
          </div>
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
  const [hiddenSeriesLabels, setHiddenSeriesLabels] = useState<Set<string>>(new Set())
  const { zoom, isDragging, onPointerDown, resetZoom } = useChartDragZoom()
  const visibleWindow = resolveVisibleSampleWindow(samples.length, zoom.xZoom)
  const seriesEntries = series.map((seriesConfig) => ({
    ...seriesConfig,
    entries: buildMetricEntries(samples, seriesConfig.accessor)
  }))
  const visibleSeriesEntries = seriesEntries.filter((seriesConfig) => !hiddenSeriesLabels.has(seriesConfig.label))
  const visibleWindowSeriesEntries = visibleSeriesEntries.map((seriesConfig) => ({
    ...seriesConfig,
    entries: seriesConfig.entries.filter((entry) => entry.index >= visibleWindow.startIndex)
  }))
  const hasRenderableSeries = visibleWindowSeriesEntries.some((seriesConfig) => seriesConfig.entries.length >= 2)
  const dataValues = visibleWindowSeriesEntries.flatMap((seriesConfig) =>
    seriesConfig.entries.map((entry) => entry.value)
  )
  const tickValuesAll = (yTicks ?? []).filter((value) => Number.isFinite(value))
  const min = dataValues.length
    ? Math.min(...dataValues, ...tickValuesAll)
    : tickValuesAll.length
      ? Math.min(...tickValuesAll)
      : 0
  const max = dataValues.length
    ? Math.max(...dataValues, ...tickValuesAll)
    : tickValuesAll.length
      ? Math.max(...tickValuesAll)
      : 0
  const topPaddingRatio = sanitizePaddingRatio(yPadding?.topRatio)
  const bottomPaddingRatio = sanitizePaddingRatio(yPadding?.bottomRatio)
  const rawMin = Number.isFinite(min) ? min : 0
  const rawMax = Number.isFinite(max) ? max : rawMin + 1
  const rawRange = Math.max(1, rawMax - rawMin)
  const baseAxisMin = rawMin - rawRange * bottomPaddingRatio
  const baseAxisMax = Math.max(baseAxisMin + 1, rawMax + rawRange * topPaddingRatio)
  const { axisMin, axisMax } = applyYAxisZoom(baseAxisMin, baseAxisMax, zoom.yZoom)
  const tickValues = tickValuesAll.filter((tick) => tick >= axisMin && tick <= axisMax)
  const minorTickValues = resolveMinorYAxisTicks(tickValuesAll, axisMin, axisMax, zoom.yZoom)
  const minorLabelTickValues = resolveMinorYAxisLabelTicks(tickValues, minorTickValues, axisMin, axisMax, zoom.yZoom)
  const denominator = Math.max(1, visibleWindow.count - 1)
  const toChartY = (value: number) => ((axisMax - value) / (axisMax - axisMin)) * CHART_VIEWBOX_HEIGHT
  const toPolylinePoints = (entries: MetricChartEntry[]) =>
    entries
      .map((entry) => {
        const x = ((entry.index - visibleWindow.startIndex) / denominator) * CHART_VIEWBOX_WIDTH
        const y = toChartY(entry.value)
        return `${x.toFixed(2)},${y.toFixed(2)}`
      })
      .join(' ')
  const toAreaPoints = (entries: MetricChartEntry[]) => {
    if (entries.length < 2) {
      return ''
    }
    const baselineY = CHART_VIEWBOX_HEIGHT.toFixed(2)
    const startX = (((entries[0].index - visibleWindow.startIndex) / denominator) * CHART_VIEWBOX_WIDTH).toFixed(2)
    const endX = (
      ((entries[entries.length - 1].index - visibleWindow.startIndex) / denominator) *
      CHART_VIEWBOX_WIDTH
    ).toFixed(2)
    return `${startX},${baselineY} ${toPolylinePoints(entries)} ${endX},${baselineY}`
  }
  const visibleSeries = visibleWindowSeriesEntries.filter((seriesConfig) => seriesConfig.entries.length > 0)
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
        <div className="flex flex-wrap items-center gap-2">
          <h4 className="text-xs font-semibold text-blue-100">{title}</h4>
          {(zoom.xZoom !== 1 || zoom.yZoom !== 1) && (
            <button
              type="button"
              onClick={resetZoom}
              className="rounded border border-white/15 bg-white/5 px-1.5 py-0.5 text-[10px] text-blue-100 hover:bg-white/10"
            >
              Reset
            </button>
          )}
        </div>
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
        <div
          className={`overflow-x-auto select-none ${isDragging ? 'cursor-grabbing' : 'cursor-grab'}`}
          onPointerDown={onPointerDown}
          title="Drag up/right to zoom in, down/left to zoom out"
        >
          <svg
            viewBox={`0 0 ${CHART_VIEWBOX_WIDTH} ${CHART_VIEWBOX_HEIGHT}`}
            className={`block w-full ${chartHeightClass} overflow-visible rounded bg-black/20`}
          >
            {minorTickValues.map((tick) => {
              const y = toChartY(tick)
              return (
                <line
                  key={`${title}-minor-tick-${tick}`}
                  x1={0}
                  y1={y}
                  x2={CHART_VIEWBOX_WIDTH}
                  y2={y}
                  stroke="rgba(148, 163, 184, 0.14)"
                  strokeDasharray="0.8 1.6"
                />
              )
            })}
            {minorLabelTickValues.map((tick) => {
              const y = toChartY(tick)
              return (
                <text
                  key={`${title}-minor-label-${tick}`}
                  x={1.2}
                  y={Math.max(2.3, y - 0.45)}
                  fontSize={2.05}
                  fill="rgba(191, 219, 254, 0.52)"
                >
                  {yTickLabelFormatter ? yTickLabelFormatter(tick) : formatMetricValue(tick, 0)}
                </text>
              )
            })}
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
                    {yTickLabelFormatter ? yTickLabelFormatter(tick) : formatMetricValue(tick, 0)}
                  </text>
                </g>
              )
            })}
            {visibleWindowSeriesEntries.map((seriesConfig) => {
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
                    vectorEffect="non-scaling-stroke"
                  />
                </g>
              )
            })}
          </svg>
        </div>
      ) : (
        <div
          className={`overflow-x-auto select-none ${isDragging ? 'cursor-grabbing' : 'cursor-grab'}`}
          onPointerDown={onPointerDown}
          title="Drag up/right to zoom in, down/left to zoom out"
        >
          <div
            className={`flex ${chartHeightClass} items-center justify-center rounded bg-black/20 text-xs text-blue-200`}
          >
            No data
          </div>
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
  maxTickCount?: number,
  rangeMode: 'zeroBased' | 'dataFocused' = 'zeroBased'
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
