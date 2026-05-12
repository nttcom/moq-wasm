import { MoqtClientWrapper } from '@moqt/moqtClient'
import { parse_msf_catalog_json } from '../../pkg/moqt_client_wasm'
import type { MOQTClient } from '../../pkg/moqt_client_wasm'

const moqtClient = new MoqtClientWrapper()
let audioDecoderWorker: Worker | null = null
const videoDecoderWorker = new Worker(new URL('../../utils/media/decoders/videoDecoder.ts', import.meta.url), {
  type: 'module'
})

type AudioDecoderWorkerMessage =
  | { type: 'audioData'; audioData: AudioData }
  | { type: 'bitrate'; media: 'audio'; kbps: number }
  | { type: 'receiveLatency'; media: 'audio'; ms: number }
  | { type: 'renderingLatency'; media: 'audio'; ms: number }
  | { type: 'bufferedObject'; media: 'audio'; groupId: bigint; objectId: bigint }

type VideoDecoderWorkerMessage =
  | { type: 'frame'; frame: VideoFrame }
  | { type: 'bitrate'; kbps: number }
  | { type: 'configError'; media: 'video'; reason: string; config: VideoDecoderConfig }
  | { type: 'receiveLatency'; media: 'video'; ms: number }
  | { type: 'renderingLatency'; media: 'video'; ms: number }
  | {
      type: 'timing'
      media: 'video'
      receiveToDecodeMs: number | null
      receiveToRenderMs: number | null
    }
  | {
      type: 'jitterBufferActivity'
      media: 'video'
      event: 'push' | 'pop'
      bufferedFrames: number
      capacityFrames: number
    }
  | {
      type: 'pacing'
      media: 'video'
      intervalMs: number
      effectiveIntervalMs: number
      bufferedFrames: number
      targetFrames: number
      lastReason?: string
      action?: string
      detailMs?: number
    }

type JitterBufferSnapshot = {
  bufferedFrames: number
  capacityFrames: number
  lastEvent: 'push' | 'pop'
  sequence: number
}

type VideoPacingConfigInput = {
  preset?: VideoPacingPreset
  pipeline?: VideoPacingPipeline
  fallbackIntervalMs?: number
  lateThresholdMs?: number
  maxWaitMs?: number
  reportIntervalMs?: number
  targetLatencyMs?: number
}

type VideoPacingPreset = 'disabled' | 'onvif' | 'call'
type VideoPacingPipeline = 'buffer-pacing-decode'

type VideoDecoderRuntimeConfig = {
  pacing: VideoPacingConfigInput
}

type StatsSnapshot = {
  bitrateKbps: number | null
  receiveLatencyMs: number | null
  renderingLatencyMs: number | null
  receiveToDecodeMs: number | null
  receiveToRenderMs: number | null
  pacingEffectiveIntervalMs: number | null
  bufferedFrames: number | null
  targetFrames: number | null
}

type StatsSample = StatsSnapshot & { tsMs: number }

type StatsChartConfig = {
  key: string
  title: string
  unit: string
  color: string
  accessor: (sample: StatsSample) => number | null
}

type CommandKind = 'absolute' | 'relative' | 'continuous' | 'stop' | 'center'

type MsfTrack = {
  namespace?: string
  name: string
  packaging: string
  eventType?: string
  role?: string
  isLive: boolean
  targetLatency?: number
  label?: string
  renderGroup?: number
  altGroup?: number
  initData?: string
  depends?: string[]
  temporalId?: number
  spatialId?: number
  codec?: string
  mimeType?: string
  framerate?: number
  timescale?: number
  bitrate?: number
  width?: number
  height?: number
  samplerate?: number
  channelConfig?: string
  displayWidth?: number
  displayHeight?: number
  lang?: string
  parentName?: string
  trackDuration?: number
}

type MsfCatalog = {
  version?: number
  deltaUpdate?: boolean
  addTracks?: MsfTrack[]
  removeTracks?: Array<{ namespace?: string; name: string }>
  cloneTracks?: MsfTrack[]
  generatedAt?: number
  isComplete?: boolean
  tracks?: MsfTrack[]
}

type CatalogTrack = {
  name: string
  label: string
  namespace?: string
  codec?: string
  resolution?: string
  role?: 'video' | 'audio' | string
  packaging?: string
  isLive?: boolean
  targetLatency?: number
  framerate?: number
  bitrate?: number
  samplerate?: number
  channelConfig?: string
}

type FieldKey = 'pan' | 'tilt' | 'zoom' | 'speed'

const COMMANDS: Array<{ type: CommandKind; label: string; disabled?: FieldKey[] }> = [
  { type: 'absolute', label: 'AbsoluteMove' },
  { type: 'relative', label: 'RelativeMove' },
  { type: 'continuous', label: 'ContinuousMove' },
  { type: 'stop', label: 'Stop', disabled: ['pan', 'tilt', 'zoom', 'speed'] },
  { type: 'center', label: 'Center', disabled: ['pan', 'tilt', 'zoom'] }
]

const FIELD_LABELS: Record<FieldKey, string> = {
  pan: 'Pan',
  tilt: 'Tilt',
  zoom: 'Zoom',
  speed: 'Speed'
}

const DEFAULT_VALUES: Record<FieldKey, number> = {
  pan: 0,
  tilt: 0,
  zoom: 0,
  speed: 1
}

const JITTER_VISIBLE_BLOCKS = 30
const JITTER_HIGHLIGHT_MS = 220
function createVideoPacingPresetConfig(preset: VideoPacingPreset): VideoPacingConfigInput {
  if (preset === 'disabled') {
    return {
      ...createVideoPacingPresetConfig('onvif'),
      preset: 'disabled',
      pipeline: 'buffer-pacing-decode',
      targetLatencyMs: 0
    }
  }
  if (preset === 'onvif') {
    return {
      preset: 'onvif',
      pipeline: 'buffer-pacing-decode',
      fallbackIntervalMs: 33,
      lateThresholdMs: 120,
      maxWaitMs: 2000,
      reportIntervalMs: 500,
      targetLatencyMs: 250
    }
  }
  return {
    ...createVideoPacingPresetConfig('onvif'),
    preset: 'call',
    pipeline: 'buffer-pacing-decode',
    targetLatencyMs: 250,
    maxWaitMs: 1000,
    lateThresholdMs: 500
  }
}
const DEFAULT_VIDEO_PACING_CONFIG: VideoPacingConfigInput = createVideoPacingPresetConfig('onvif')
const DEFAULT_VIDEO_DECODER_CONFIG: VideoDecoderRuntimeConfig = {
  pacing: { ...DEFAULT_VIDEO_PACING_CONFIG }
}
const STATS_SAMPLE_INTERVAL_MS = 250
const STATS_MAX_SAMPLES = 240
const STATS_CHARTS: StatsChartConfig[] = [
  {
    key: 'bitrate',
    title: 'Video Bitrate',
    unit: 'kbps',
    color: '#6fe6ff',
    accessor: (sample) => sample.bitrateKbps
  },
  {
    key: 'receive-latency',
    title: 'Receive Latency',
    unit: 'ms',
    color: '#77f0b8',
    accessor: (sample) => sample.receiveLatencyMs
  },
  {
    key: 'receive-to-render',
    title: 'Receive To Render',
    unit: 'ms',
    color: '#ffd66f',
    accessor: (sample) => sample.receiveToRenderMs
  },
  {
    key: 'receive-to-decode',
    title: 'Receive To Decode',
    unit: 'ms',
    color: '#ff9fb3',
    accessor: (sample) => sample.receiveToDecodeMs
  },
  {
    key: 'buffered-frames',
    title: 'Buffered Frames',
    unit: 'frames',
    color: '#9ac6ff',
    accessor: (sample) => sample.bufferedFrames
  }
]

function cloneVideoDecoderConfig(config: VideoDecoderRuntimeConfig): VideoDecoderRuntimeConfig {
  return {
    ...config,
    pacing: { ...config.pacing }
  }
}

function createEmptyStatsSnapshot(): StatsSnapshot {
  return {
    bitrateKbps: null,
    receiveLatencyMs: null,
    renderingLatencyMs: null,
    receiveToDecodeMs: null,
    receiveToRenderMs: null,
    pacingEffectiveIntervalMs: null,
    bufferedFrames: null,
    targetFrames: null
  }
}

let currentVideoDecoderConfig: VideoDecoderRuntimeConfig = cloneVideoDecoderConfig(DEFAULT_VIDEO_DECODER_CONFIG)
let audioWorkerInitialized = false
let videoWorkerInitialized = false
let commandTrackAlias: bigint | null = null
let commandObjectId = 0n
let catalogTrackAlias: bigint | null = null
let catalogTracks: CatalogTrack[] = []
let audioSubscribed = false
let videoSubscribed = false
let selectedAudioTrack: string | null = null
let selectedVideoTrack: string | null = null
let audioPlaybackStarted = false
let jitterSnapshot: JitterBufferSnapshot | null = null
let jitterHighlightTimer: number | null = null
let latestStats: StatsSnapshot = createEmptyStatsSnapshot()
let statsSamples: StatsSample[] = []
let lastStatsSampleMs = 0

const jitterVisualizer = document.getElementById('jitter-visualizer')
const jitterMeta = document.getElementById('jitter-meta')
const pacingMeta = document.getElementById('pacing-meta')
const timingMeta = document.getElementById('timing-meta')
const pacingPresetInput = document.getElementById('video-pacing-preset') as HTMLSelectElement | null
const statsBtn = document.getElementById('stats-btn') as HTMLButtonElement | null
const statsModal = document.getElementById('stats-modal')
const statsCloseBtn = document.getElementById('stats-close-btn') as HTMLButtonElement | null
const statsSummary = document.getElementById('stats-summary')
const statsCharts = document.getElementById('stats-charts')
const jitterBlocks: HTMLElement[] = []

if (pacingPresetInput) {
  pacingPresetInput.value = currentVideoDecoderConfig.pacing.preset ?? 'onvif'
}

if (jitterVisualizer) {
  jitterVisualizer.innerHTML = ''
  for (let i = 0; i < JITTER_VISIBLE_BLOCKS; i += 1) {
    const block = document.createElement('span')
    block.className = 'jitter-block'
    jitterVisualizer.appendChild(block)
    jitterBlocks.push(block)
  }
}

function isStatsModalOpen(): boolean {
  return Boolean(statsModal?.classList.contains('open'))
}

function openStatsModal(): void {
  if (!statsModal) return
  statsModal.classList.add('open')
  statsModal.setAttribute('aria-hidden', 'false')
  renderStatsViewer()
}

function closeStatsModal(): void {
  if (!statsModal) return
  statsModal.classList.remove('open')
  statsModal.setAttribute('aria-hidden', 'true')
}

function formatStatsNumber(value: number | null, digits = 0): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return '-'
  }
  return value.toFixed(digits).replace(/\.?0+$/, '')
}

function pushStatsSample(force = false): void {
  const nowMs = performance.now()
  if (!force && nowMs - lastStatsSampleMs < STATS_SAMPLE_INTERVAL_MS) {
    return
  }
  lastStatsSampleMs = nowMs
  statsSamples.push({
    tsMs: nowMs,
    ...latestStats
  })
  if (statsSamples.length > STATS_MAX_SAMPLES) {
    statsSamples = statsSamples.slice(-STATS_MAX_SAMPLES)
  }
  if (isStatsModalOpen()) {
    renderStatsViewer()
  }
}

function applyStatsPatch(patch: Partial<StatsSnapshot>, forceSample = false): void {
  latestStats = { ...latestStats, ...patch }
  pushStatsSample(forceSample)
}

function resetStatsSamples(): void {
  latestStats = createEmptyStatsSnapshot()
  statsSamples = []
  lastStatsSampleMs = 0
  pushStatsSample(true)
}

function createSummaryItem(label: string, value: string): HTMLElement {
  const item = document.createElement('div')
  item.className = 'stats-summary-item'
  const key = document.createElement('div')
  key.className = 'stats-summary-label'
  key.textContent = label
  const val = document.createElement('div')
  val.className = 'stats-summary-value'
  val.textContent = value
  item.append(key, val)
  return item
}

function renderStatsSummary(): void {
  if (!statsSummary) {
    return
  }
  statsSummary.innerHTML = ''
  const latest = latestStats
  const bufferedLabel =
    typeof latest.bufferedFrames === 'number' && Number.isFinite(latest.bufferedFrames)
      ? `${Math.round(latest.bufferedFrames)} / ${formatStatsNumber(latest.targetFrames)}`
      : '-'
  const items: Array<{ label: string; value: string }> = [
    { label: 'Bitrate', value: `${formatStatsNumber(latest.bitrateKbps)} kbps` },
    { label: 'Receive Latency', value: `${formatStatsNumber(latest.receiveLatencyMs)} ms` },
    { label: 'Receive To Decode', value: `${formatStatsNumber(latest.receiveToDecodeMs)} ms` },
    { label: 'Receive To Render', value: `${formatStatsNumber(latest.receiveToRenderMs)} ms` },
    { label: 'Pacing Interval', value: `${formatStatsNumber(latest.pacingEffectiveIntervalMs, 1)} ms` },
    { label: 'Buffered / Target', value: bufferedLabel }
  ]
  for (const item of items) {
    statsSummary.appendChild(createSummaryItem(item.label, item.value))
  }
}

function buildLinePath(
  series: Array<number | null>,
  width: number,
  height: number,
  minY: number,
  maxY: number
): string {
  const range = Math.max(1e-6, maxY - minY)
  const maxIndex = Math.max(1, series.length - 1)
  let path = ''
  let penDown = false
  for (let i = 0; i < series.length; i += 1) {
    const value = series[i]
    if (typeof value !== 'number' || !Number.isFinite(value)) {
      penDown = false
      continue
    }
    const x = (i / maxIndex) * width
    const y = height - ((value - minY) / range) * height
    if (!penDown) {
      path += `M ${x.toFixed(2)} ${y.toFixed(2)} `
      penDown = true
    } else {
      path += `L ${x.toFixed(2)} ${y.toFixed(2)} `
    }
  }
  return path.trim()
}

function renderStatsCharts(): void {
  if (!statsCharts) {
    return
  }
  statsCharts.innerHTML = ''
  if (statsSamples.length < 2) {
    const empty = document.createElement('div')
    empty.className = 'stats-empty'
    empty.textContent = 'No stats yet'
    statsCharts.appendChild(empty)
    return
  }

  const samples = statsSamples.slice(-120)
  for (const chart of STATS_CHARTS) {
    const series = samples.map((sample) => chart.accessor(sample))
    const valid = series.filter((value): value is number => typeof value === 'number' && Number.isFinite(value))
    const card = document.createElement('div')
    card.className = 'stats-chart-card'
    const head = document.createElement('div')
    head.className = 'stats-chart-head'
    const title = document.createElement('div')
    title.className = 'stats-chart-title'
    title.textContent = chart.title
    const value = document.createElement('div')
    value.className = 'stats-chart-value'
    value.textContent = `${formatStatsNumber(valid[valid.length - 1] ?? null, chart.unit === 'ms' ? 1 : 0)} ${chart.unit}`
    head.append(title, value)
    card.appendChild(head)

    if (valid.length < 2) {
      const empty = document.createElement('div')
      empty.className = 'stats-empty'
      empty.textContent = 'Waiting for samples'
      card.appendChild(empty)
      statsCharts.appendChild(card)
      continue
    }

    const minRaw = Math.min(...valid)
    const maxRaw = Math.max(...valid)
    const margin = Math.max(1, (maxRaw - minRaw) * 0.08)
    const minY = Math.max(0, minRaw - margin)
    const maxY = maxRaw + margin
    const width = 320
    const height = 120
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg')
    svg.setAttribute('viewBox', `0 0 ${width} ${height}`)
    svg.setAttribute('class', 'stats-chart-svg')
    for (let i = 0; i <= 4; i += 1) {
      const y = (i / 4) * height
      const guide = document.createElementNS('http://www.w3.org/2000/svg', 'line')
      guide.setAttribute('x1', '0')
      guide.setAttribute('y1', y.toFixed(2))
      guide.setAttribute('x2', width.toString())
      guide.setAttribute('y2', y.toFixed(2))
      guide.setAttribute('stroke', 'rgba(255,255,255,0.10)')
      guide.setAttribute('stroke-width', '1')
      svg.appendChild(guide)
    }
    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path')
    path.setAttribute('d', buildLinePath(series, width, height, minY, maxY))
    path.setAttribute('fill', 'none')
    path.setAttribute('stroke', chart.color)
    path.setAttribute('stroke-width', '2')
    path.setAttribute('stroke-linejoin', 'round')
    path.setAttribute('stroke-linecap', 'round')
    svg.appendChild(path)
    card.appendChild(svg)

    const meta = document.createElement('div')
    meta.className = 'stats-chart-meta'
    meta.textContent = `min ${formatStatsNumber(minRaw, 1)} / max ${formatStatsNumber(maxRaw, 1)} ${chart.unit}`
    card.appendChild(meta)
    statsCharts.appendChild(card)
  }
}

function renderStatsViewer(): void {
  renderStatsSummary()
  renderStatsCharts()
}

function setupVideoDecoderWorker(): void {
  if (videoWorkerInitialized) return

  videoWorkerInitialized = true
  const videoGenerator = new MediaStreamTrackGenerator({ kind: 'video' })
  const videoWriter = videoGenerator.writable.getWriter()
  const videoStream = new MediaStream([videoGenerator])
  const videoElement = document.getElementById('video') as HTMLVideoElement | null
  if (videoElement) {
    videoElement.srcObject = videoStream
    void videoElement.play().catch((error) => {
      console.warn('[onvif][video] play failed', error)
    })
  }
  videoDecoderWorker.postMessage({ type: 'config', config: currentVideoDecoderConfig })

  videoDecoderWorker.onmessage = async (event: MessageEvent<VideoDecoderWorkerMessage>) => {
    if (event.data.type === 'bitrate') {
      applyStatsPatch({ bitrateKbps: event.data.kbps })
      return
    }
    if (event.data.type === 'configError') {
      console.error('[onvif][videoDecoder] config error', {
        reason: event.data.reason,
        config: event.data.config
      })
      return
    }
    if (event.data.type === 'receiveLatency') {
      applyStatsPatch({ receiveLatencyMs: event.data.ms })
      return
    }
    if (event.data.type === 'renderingLatency') {
      applyStatsPatch({ renderingLatencyMs: event.data.ms })
      return
    }
    if (event.data.type === 'jitterBufferActivity') {
      updateJitterSnapshot({
        bufferedFrames: event.data.bufferedFrames,
        capacityFrames: event.data.capacityFrames,
        lastEvent: event.data.event
      })
      applyStatsPatch({ bufferedFrames: event.data.bufferedFrames })
      return
    }
    if (event.data.type === 'pacing') {
      updatePacingMeta(event.data)
      applyStatsPatch({
        pacingEffectiveIntervalMs: event.data.effectiveIntervalMs,
        bufferedFrames: event.data.bufferedFrames,
        targetFrames: event.data.targetFrames
      })
      return
    }
    if (event.data.type === 'timing') {
      updateTimingMeta(event.data)
      applyStatsPatch({
        receiveToDecodeMs: event.data.receiveToDecodeMs,
        receiveToRenderMs: event.data.receiveToRenderMs
      })
      return
    }
    if (event.data.type !== 'frame') {
      return
    }
    const writerDesiredSize = videoWriter.desiredSize
    if (typeof writerDesiredSize === 'number' && writerDesiredSize <= 0) {
      console.warn('[onvif][writer] drop frame due to backpressure', {
        desiredSize: writerDesiredSize,
        width: event.data.frame.displayWidth,
        height: event.data.frame.displayHeight
      })
      event.data.frame.close()
      return
    }
    await videoWriter.ready
    await videoWriter.write(event.data.frame)
    event.data.frame.close()
  }
}

function setupAudioDecoderWorker(): void {
  if (audioWorkerInitialized) return

  audioDecoderWorker = new Worker(new URL('../../utils/media/decoders/audioDecoder.ts', import.meta.url), {
    type: 'module'
  })
  audioWorkerInitialized = true
  const audioGenerator = new MediaStreamTrackGenerator({ kind: 'audio' })
  const audioWriter = audioGenerator.writable.getWriter()
  const audioStream = new MediaStream([audioGenerator])
  const audioElement = document.getElementById('audio') as HTMLAudioElement | null
  if (audioElement) {
    audioElement.srcObject = audioStream
  }
  const worker = audioDecoderWorker

  worker.onmessage = async (event: MessageEvent<AudioDecoderWorkerMessage>) => {
    const data = event.data
    if (data.type !== 'audioData') {
      return
    }
    await audioWriter.ready
    await audioWriter.write(data.audioData)
    data.audioData.close()
    if (!audioPlaybackStarted && audioElement) {
      try {
        await audioElement.play()
        audioPlaybackStarted = true
      } catch (error) {
        console.warn('[onvif][audio] play failed', error)
      }
    }
  }
}

function setupVideoCallbacks(trackAlias: bigint): void {
  setupVideoDecoderWorker()
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (groupId, subgroupStreamObject) => {
    const payload = new Uint8Array(subgroupStreamObject.objectPayload)
    videoDecoderWorker.postMessage(
      {
        groupId,
        subgroupStreamObject: {
          subgroupId: subgroupStreamObject.subgroupId,
          objectIdDelta: subgroupStreamObject.objectIdDelta,
          objectPayloadLength: subgroupStreamObject.objectPayloadLength,
          objectPayload: payload,
          objectStatus: subgroupStreamObject.objectStatus,
          locHeader: subgroupStreamObject.locHeader
        }
      },
      [payload.buffer]
    )
  })
}

function setupAudioCallbacks(trackAlias: bigint): void {
  setupAudioDecoderWorker()
  const worker = audioDecoderWorker
  if (!worker) {
    throw new Error('audio decoder worker is not initialized')
  }
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (groupId, subgroupStreamObject) => {
    const payload = new Uint8Array(subgroupStreamObject.objectPayload)
    worker.postMessage(
      {
        groupId,
        subgroupStreamObject: {
          subgroupId: subgroupStreamObject.subgroupId,
          objectIdDelta: subgroupStreamObject.objectIdDelta,
          objectPayloadLength: subgroupStreamObject.objectPayloadLength,
          objectPayload: payload,
          objectStatus: subgroupStreamObject.objectStatus,
          locHeader: subgroupStreamObject.locHeader
        }
      },
      [payload.buffer]
    )
  })
}

function updateStatus(label: string, active: boolean): void {
  const status = document.getElementById('status')
  if (!status) return
  status.innerHTML = `<span></span>${label}`
  const dot = status.querySelector('span')
  if (dot instanceof HTMLElement) {
    dot.style.background = active ? '#2ec4b6' : '#ff9f1c'
  }
}

function updateCommandAlias(alias: bigint | null): void {
  const element = document.getElementById('command-alias')
  if (!element) return
  element.textContent = alias === null ? '-' : alias.toString()
}

function updateCatalogAlias(alias: bigint | null): void {
  const element = document.getElementById('catalog-alias')
  if (!element) return
  element.textContent = alias === null ? '-' : alias.toString()
}

function updateSelectedVideoTrackLabel(track: string | null): void {
  const element = document.getElementById('selected-video-track')
  if (!element) return
  element.textContent = track ?? '-'
}

function updateSelectedAudioTrackLabel(track: string | null): void {
  const element = document.getElementById('selected-audio-track')
  if (!element) return
  element.textContent = track ?? '-'
}

function updateJitterSnapshot(update: { bufferedFrames: number; capacityFrames: number; lastEvent: 'push' | 'pop' }) {
  const sequence = (jitterSnapshot?.sequence ?? 0) + 1
  jitterSnapshot = {
    bufferedFrames: update.bufferedFrames,
    capacityFrames: update.capacityFrames,
    lastEvent: update.lastEvent,
    sequence
  }
  renderJitterVisualizer(jitterSnapshot)
}

function renderJitterVisualizer(snapshot: JitterBufferSnapshot | null): void {
  if (!jitterBlocks.length) {
    return
  }
  const bufferedFrames = snapshot ? Math.max(0, Math.floor(snapshot.bufferedFrames)) : 0
  const filledBlocks = Math.min(JITTER_VISIBLE_BLOCKS, bufferedFrames)
  const firstFilledIndex = JITTER_VISIBLE_BLOCKS - filledBlocks

  for (let i = 0; i < jitterBlocks.length; i += 1) {
    const block = jitterBlocks[i]
    block.classList.toggle('filled', i >= firstFilledIndex)
    block.classList.remove('highlight-push', 'highlight-pop')
  }

  if (snapshot) {
    if (snapshot.lastEvent === 'push' && filledBlocks > 0) {
      jitterBlocks[firstFilledIndex]?.classList.add('highlight-push')
    }
    if (snapshot.lastEvent === 'pop' && firstFilledIndex > 0) {
      jitterBlocks[firstFilledIndex - 1]?.classList.add('highlight-pop')
    }
    if (jitterMeta) {
      jitterMeta.textContent = `buffered ${Math.round(snapshot.bufferedFrames)} frames`
    }
    if (jitterHighlightTimer !== null) {
      window.clearTimeout(jitterHighlightTimer)
    }
    jitterHighlightTimer = window.setTimeout(() => {
      jitterBlocks.forEach((block) => {
        block.classList.remove('highlight-push', 'highlight-pop')
      })
    }, JITTER_HIGHLIGHT_MS)
  } else if (jitterMeta) {
    jitterMeta.textContent = 'buffered 0 frames'
  }
}

function updatePacingMeta(data: {
  intervalMs: number
  effectiveIntervalMs: number
  bufferedFrames: number
  targetFrames: number
  lastReason?: string
  detailMs?: number
}): void {
  if (!pacingMeta) {
    return
  }
  const intervalText = Number.isFinite(data.intervalMs) ? data.intervalMs.toFixed(1) : '-'
  const effectiveIntervalText = Number.isFinite(data.effectiveIntervalMs) ? data.effectiveIntervalMs.toFixed(1) : '-'
  const bufferText = Number.isFinite(data.bufferedFrames) ? Math.round(data.bufferedFrames) : '-'
  const targetText = Number.isFinite(data.targetFrames) ? Math.round(data.targetFrames) : '-'
  const reason = data.lastReason ?? '-'
  const detail =
    typeof data.detailMs === 'number' && Number.isFinite(data.detailMs) ? `, detail ${Math.round(data.detailMs)}ms` : ''
  pacingMeta.textContent =
    `pacing: interval ${intervalText}ms (eff ${effectiveIntervalText}ms)\n` +
    `buffer ${bufferText}/${targetText}, reason ${reason}${detail}`
}

function updateTimingMeta(data: { receiveToDecodeMs: number | null; receiveToRenderMs: number | null }): void {
  if (!timingMeta) {
    return
  }
  const receiveToDecodeText =
    typeof data.receiveToDecodeMs === 'number' && Number.isFinite(data.receiveToDecodeMs)
      ? `${Math.round(data.receiveToDecodeMs)}ms`
      : '-'
  const receiveToRenderText =
    typeof data.receiveToRenderMs === 'number' && Number.isFinite(data.receiveToRenderMs)
      ? `${Math.round(data.receiveToRenderMs)}ms`
      : '-'
  timingMeta.textContent = `receiveToDecode ${receiveToDecodeText} / receiveToRender ${receiveToRenderText}`
}

function ensureClient(): MOQTClient {
  const client = moqtClient.getRawClient()
  if (!client) {
    throw new Error('MOQT client not connected')
  }
  return client
}

function parseNamespace(value: string): string[] {
  return value
    .split('/')
    .map((part) => part.trim())
    .filter((part) => part.length > 0)
}

function parseBigInt(value: string): bigint {
  try {
    return BigInt(value)
  } catch {
    return 0n
  }
}

function applyVideoDecoderSettings(): void {
  const selectedPreset: VideoPacingPreset =
    pacingPresetInput?.value === 'disabled' ||
    pacingPresetInput?.value === 'call' ||
    pacingPresetInput?.value === 'onvif'
      ? pacingPresetInput.value
      : 'onvif'
  let pacingConfig: VideoPacingConfigInput = createVideoPacingPresetConfig(selectedPreset)
  pacingConfig.pipeline = 'buffer-pacing-decode'
  if (pacingPresetInput) {
    pacingPresetInput.value = selectedPreset
  }

  currentVideoDecoderConfig = {
    ...currentVideoDecoderConfig,
    pacing: { ...pacingConfig }
  }

  if (videoWorkerInitialized) {
    videoDecoderWorker.postMessage({ type: 'config', config: currentVideoDecoderConfig })
  }
}

function applySelectedVideoTrack(track: string): void {
  selectedVideoTrack = track
  updateSelectedVideoTrackLabel(track)
  notifyDecoderFromCatalog(track)
}

function applySelectedAudioTrack(track: string): void {
  selectedAudioTrack = track
  updateSelectedAudioTrackLabel(track)
}

function notifyDecoderFromCatalog(trackName: string | null): void {
  if (!trackName) return
  const track = catalogTracks.find((entry) => entry.name === trackName && entry.role === 'video')
  if (!track?.codec && typeof track?.framerate !== 'number') return
  videoDecoderWorker.postMessage({ type: 'catalog', codec: track.codec, framerate: track.framerate })
}

function appendMeta(container: HTMLElement, label: string, value?: string): void {
  if (!value) return
  const item = document.createElement('span')
  item.textContent = `${label}=${value}`
  container.appendChild(item)
}

function formatNumber(value?: number): string | undefined {
  if (typeof value !== 'number') return undefined
  return value.toFixed(2).replace(/\.?0+$/, '')
}

function formatBitrate(value?: number): string | undefined {
  if (typeof value !== 'number') return undefined
  if (value >= 1_000_000) {
    const mbps = (value / 1_000_000).toFixed(2).replace(/\.?0+$/, '')
    return `${mbps}Mbps`
  }
  if (value >= 1_000) {
    return `${Math.round(value / 1_000)}kbps`
  }
  return `${Math.round(value)}bps`
}

function formatLatency(value?: number): string | undefined {
  if (typeof value !== 'number') return undefined
  return `${Math.round(value)}ms`
}

function formatLive(value?: boolean): string | undefined {
  if (typeof value !== 'boolean') return undefined
  return value ? 'live' : 'ended'
}

function getCatalogTracksByRole(role: 'video' | 'audio'): CatalogTrack[] {
  return catalogTracks.filter((track) => track.role === role)
}

function ensureSelectedCatalogTrack(role: 'video' | 'audio'): string | null {
  const tracks = getCatalogTracksByRole(role)
  const currentTrack = role === 'video' ? selectedVideoTrack : selectedAudioTrack
  if (currentTrack && tracks.some((track) => track.name === currentTrack)) {
    return currentTrack
  }
  return tracks[0]?.name ?? null
}

function renderCatalogRoleSection(
  list: HTMLElement,
  role: 'video' | 'audio',
  title: string,
  selectedTrack: string | null
): void {
  const tracks = getCatalogTracksByRole(role)
  const section = document.createElement('div')
  section.className = 'catalog-section'

  const header = document.createElement('div')
  header.className = 'command-title'
  header.textContent = title
  section.appendChild(header)

  if (!tracks.length) {
    const empty = document.createElement('div')
    empty.className = 'muted'
    empty.textContent = `No ${role} tracks in catalog.`
    section.appendChild(empty)
    list.appendChild(section)
    return
  }

  for (const [index, track] of tracks.entries()) {
    const item = document.createElement('label')
    item.className = 'catalog-item'

    const itemHeader = document.createElement('div')
    itemHeader.className = 'command-title'
    itemHeader.textContent = track.label || `${title} ${index + 1}`

    const subtitle = document.createElement('div')
    subtitle.className = 'catalog-subtitle'
    subtitle.textContent = track.namespace ? `${track.name} @ ${track.namespace}` : track.name

    const inputRow = document.createElement('div')
    const radio = document.createElement('input')
    radio.type = 'radio'
    radio.name = `catalog-profile-${role}`
    radio.value = track.name
    radio.checked = selectedTrack === track.name
    radio.addEventListener('change', () => {
      if (role === 'video') {
        applySelectedVideoTrack(track.name)
      } else {
        applySelectedAudioTrack(track.name)
      }
    })
    const trackLabel = document.createElement('span')
    trackLabel.textContent = track.name
    inputRow.append(radio, trackLabel)

    const meta = document.createElement('div')
    meta.className = 'catalog-meta'
    appendMeta(meta, 'role', track.role)
    appendMeta(meta, 'packaging', track.packaging)
    appendMeta(meta, 'status', formatLive(track.isLive))
    appendMeta(meta, 'resolution', track.resolution)
    appendMeta(meta, 'codec', track.codec)
    appendMeta(meta, 'fps', formatNumber(track.framerate))
    appendMeta(meta, 'bitrate', formatBitrate(track.bitrate))
    appendMeta(meta, 'samplerate', typeof track.samplerate === 'number' ? `${track.samplerate}Hz` : undefined)
    appendMeta(meta, 'channels', track.channelConfig)
    appendMeta(meta, 'latency', formatLatency(track.targetLatency))

    item.append(itemHeader, subtitle, inputRow, meta)
    section.appendChild(item)
  }

  list.appendChild(section)
}

function renderCatalogList(): void {
  const list = document.getElementById('catalog-list')
  if (!list) return
  list.innerHTML = ''
  if (catalogTracks.length === 0) {
    const empty = document.createElement('div')
    empty.className = 'muted'
    empty.textContent = 'No catalog data yet.'
    list.appendChild(empty)
    updateSelectedVideoTrackLabel(selectedVideoTrack)
    updateSelectedAudioTrackLabel(selectedAudioTrack)
    return
  }

  const nextVideoTrack = ensureSelectedCatalogTrack('video')
  if (nextVideoTrack) {
    applySelectedVideoTrack(nextVideoTrack)
  } else {
    selectedVideoTrack = null
    updateSelectedVideoTrackLabel(null)
  }

  const nextAudioTrack = ensureSelectedCatalogTrack('audio')
  if (nextAudioTrack) {
    applySelectedAudioTrack(nextAudioTrack)
  } else {
    selectedAudioTrack = null
    updateSelectedAudioTrackLabel(null)
  }

  renderCatalogRoleSection(list, 'video', 'Video Tracks', selectedVideoTrack)
  renderCatalogRoleSection(list, 'audio', 'Audio Tracks', selectedAudioTrack)

  updateSelectedVideoTrackLabel(selectedVideoTrack)
  updateSelectedAudioTrackLabel(selectedAudioTrack)
}

function setupCatalogCallbacks(trackAlias: bigint): void {
  moqtClient.setOnSubgroupObjectHandler(trackAlias, (_groupId, subgroupStreamObject) => {
    const payloadBytes = new Uint8Array(subgroupStreamObject.objectPayload)
    const payload = new TextDecoder().decode(payloadBytes)
    try {
      const parsed = parse_msf_catalog_json(payload) as MsfCatalog
      catalogTracks = buildCatalogTracks(parsed)
      renderCatalogList()
      notifyDecoderFromCatalog(selectedVideoTrack)
      updateStatus('catalog received', true)
    } catch (err) {
      console.error(err)
      updateStatus('catalog parse failed', false)
    }
  })
}

function buildCatalogTracks(catalog: MsfCatalog): CatalogTrack[] {
  const tracks = catalog.tracks ?? catalog.addTracks ?? []
  return tracks.map((track) => {
    const label = track.label ?? track.name
    const resolution =
      typeof track.width === 'number' && typeof track.height === 'number' ? `${track.width}x${track.height}` : undefined
    return {
      name: track.name,
      label,
      namespace: track.namespace,
      codec: track.codec,
      resolution,
      role: track.role,
      packaging: track.packaging,
      isLive: track.isLive,
      targetLatency: track.targetLatency,
      framerate: track.framerate,
      bitrate: track.bitrate,
      samplerate: track.samplerate,
      channelConfig: track.channelConfig
    }
  })
}

function readField(row: HTMLElement, field: FieldKey): number {
  const input = row.querySelector<HTMLInputElement>(`input[data-field="${field}"]`)
  if (!input) return DEFAULT_VALUES[field]
  const raw = Number(input.value)
  const value = Number.isFinite(raw) ? raw : DEFAULT_VALUES[field]
  const rounded = Math.round(value * 10) / 10
  input.value = rounded.toFixed(1)
  return rounded
}

async function sendCommand(command: CommandKind, row: HTMLElement): Promise<void> {
  const client = ensureClient()
  if (commandTrackAlias === null) {
    updateStatus('command track not subscribed', false)
    return
  }

  const pan = readField(row, 'pan')
  const tilt = readField(row, 'tilt')
  const zoom = readField(row, 'zoom')
  const speed = readField(row, 'speed')

  const payload =
    command === 'stop'
      ? { type: 'stop' }
      : command === 'center'
        ? { type: 'center', speed }
        : { type: command, pan, tilt, zoom, speed }

  const bytes = new TextEncoder().encode(JSON.stringify(payload))
  await client.sendObjectDatagram(commandTrackAlias, 0n, commandObjectId, 0, bytes, undefined)
  commandObjectId += 1n
  updateStatus(`sent ${command}`, true)
}

function buildCommandUI(): void {
  const grid = document.getElementById('command-grid')
  if (!grid) return
  grid.innerHTML = ''

  for (const command of COMMANDS) {
    const row = document.createElement('div')
    row.className = 'command'

    const header = document.createElement('div')
    header.className = 'command-header'

    const title = document.createElement('div')
    title.className = 'command-title'
    title.textContent = command.label

    const button = document.createElement('button')
    button.type = 'button'
    button.textContent = 'Send'
    button.addEventListener('click', () => {
      sendCommand(command.type, row).catch((err) => {
        console.error(err)
        updateStatus('command send failed', false)
      })
    })

    header.append(title, button)

    const inputs = document.createElement('div')
    inputs.className = 'command-inputs'

    const disabledFields = new Set(command.disabled ?? [])
    ;(Object.keys(FIELD_LABELS) as FieldKey[]).forEach((field) => {
      const label = document.createElement('label')
      label.textContent = FIELD_LABELS[field]

      const input = document.createElement('input')
      input.type = 'number'
      input.step = '0.1'
      input.inputMode = 'decimal'
      input.value = DEFAULT_VALUES[field].toFixed(1)
      input.dataset.field = field
      if (disabledFields.has(field)) {
        input.disabled = true
      }
      label.appendChild(input)
      inputs.appendChild(label)
    })

    row.append(header, inputs)
    grid.appendChild(row)
  }
}

function setupUrlPresets(): void {
  const urlInput = document.getElementById('moqt-url') as HTMLInputElement | null
  if (!urlInput) return
  const buttons = document.querySelectorAll<HTMLButtonElement>('button[data-url]')
  buttons.forEach((button) => {
    button.addEventListener('click', () => {
      const url = button.dataset.url
      if (url) {
        urlInput.value = url
      }
    })
  })
}

function openSettingsModal(): void {
  const modal = document.getElementById('settings-modal')
  if (!modal) return
  modal.classList.add('open')
  modal.setAttribute('aria-hidden', 'false')
}

function closeSettingsModal(): void {
  const modal = document.getElementById('settings-modal')
  if (!modal) return
  modal.classList.remove('open')
  modal.setAttribute('aria-hidden', 'true')
}

async function subscribeCatalog(): Promise<void> {
  ensureClient()
  const subscribeNamespace = parseNamespace((document.getElementById('subscribe-namespace') as HTMLInputElement).value)
  const catalogTrack = (document.getElementById('catalog-track') as HTMLInputElement).value
  const authInfo = (document.getElementById('auth-info') as HTMLInputElement).value
  const catalogSubscribeId = parseBigInt((document.getElementById('catalog-subscribe-id') as HTMLInputElement).value)

  if (!subscribeNamespace.length) {
    updateStatus('namespace required', false)
    return
  }

  const catalogAlias = await moqtClient.subscribe(catalogSubscribeId, subscribeNamespace, catalogTrack, authInfo)
  ;(document.getElementById('catalog-track-alias') as HTMLInputElement).value = catalogAlias.toString()
  setupCatalogCallbacks(catalogAlias)
  catalogTrackAlias = catalogAlias
  updateCatalogAlias(catalogTrackAlias)
  updateStatus('catalog subscribed', true)
}

async function subscribeSelectedVideo(): Promise<void> {
  if (videoSubscribed) {
    updateStatus('video already subscribed', false)
    return
  }
  ensureClient()
  const subscribeNamespace = parseNamespace((document.getElementById('subscribe-namespace') as HTMLInputElement).value)
  const videoTrack = selectedVideoTrack?.trim() ?? ''
  const authInfo = (document.getElementById('auth-info') as HTMLInputElement).value
  const videoSubscribeId = parseBigInt((document.getElementById('video-subscribe-id') as HTMLInputElement).value)

  if (!subscribeNamespace.length || !videoTrack) {
    updateStatus('video track required', false)
    return
  }

  const videoTrackAlias = await moqtClient.subscribe(videoSubscribeId, subscribeNamespace, videoTrack, authInfo)
  ;(document.getElementById('video-track-alias') as HTMLInputElement).value = videoTrackAlias.toString()
  setupVideoCallbacks(videoTrackAlias)
  videoSubscribed = true
  updateStatus(`video subscribed: ${videoTrack}`, true)
}

async function subscribeSelectedAudio(): Promise<void> {
  if (audioSubscribed) {
    updateStatus('audio already subscribed', false)
    return
  }
  ensureClient()
  const subscribeNamespace = parseNamespace((document.getElementById('subscribe-namespace') as HTMLInputElement).value)
  const audioTrack = selectedAudioTrack?.trim() ?? ''
  const authInfo = (document.getElementById('auth-info') as HTMLInputElement).value
  const audioSubscribeId = parseBigInt((document.getElementById('audio-subscribe-id') as HTMLInputElement).value)

  if (!subscribeNamespace.length || !audioTrack) {
    updateStatus('audio track required', false)
    return
  }

  const audioTrackAlias = await moqtClient.subscribe(audioSubscribeId, subscribeNamespace, audioTrack, authInfo)
  ;(document.getElementById('audio-track-alias') as HTMLInputElement).value = audioTrackAlias.toString()
  setupAudioCallbacks(audioTrackAlias)
  audioSubscribed = true
  updateStatus(`audio subscribed: ${audioTrack}`, true)
}

async function connect(): Promise<void> {
  const url = (document.getElementById('moqt-url') as HTMLInputElement).value
  const publishNamespace = parseNamespace((document.getElementById('publish-namespace') as HTMLInputElement).value)
  const subscribeNamespace = parseNamespace((document.getElementById('subscribe-namespace') as HTMLInputElement).value)
  const publishNamespaceLabel = publishNamespace.join('/')
  const commandTrack = (document.getElementById('command-track') as HTMLInputElement).value
  const authInfo = (document.getElementById('auth-info') as HTMLInputElement).value
  const maxSubscribeId = parseBigInt((document.getElementById('max-subscribe-id') as HTMLInputElement).value)

  if (!publishNamespace.length || !subscribeNamespace.length) {
    updateStatus('namespace required', false)
    return
  }

  catalogTracks = []
  audioSubscribed = false
  videoSubscribed = false
  catalogTrackAlias = null
  selectedAudioTrack = null
  selectedVideoTrack = null
  audioPlaybackStarted = false
  resetStatsSamples()
  renderCatalogList()
  updateCatalogAlias(catalogTrackAlias)
  updateSelectedAudioTrackLabel(selectedAudioTrack)
  updateSelectedVideoTrackLabel(selectedVideoTrack)

  updateStatus('connecting', true)
  await moqtClient.connect(url, { sendSetup: false })
  moqtClient.setOnConnectionClosedHandler(() => {
    updateStatus('disconnected', false)
  })

  await moqtClient.sendClientSetup(new BigUint64Array([0xff00000en]), maxSubscribeId)

  moqtClient.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
    if (!isSuccess) {
      await respondError(BigInt(code), 'subscribe rejected')
      return
    }

    if (subscribe.trackName !== commandTrack || subscribe.trackNamespace.join('/') !== publishNamespaceLabel) {
      await respondError(404n, 'unknown track')
      return
    }

    commandTrackAlias = await respondOk(0n)
    updateCommandAlias(commandTrackAlias)
  })

  await moqtClient.publishNamespace(publishNamespace, authInfo)
  updateStatus('connected', true)
}

async function disconnect(): Promise<void> {
  await moqtClient.disconnect()
  commandTrackAlias = null
  commandObjectId = 0n
  catalogTrackAlias = null
  catalogTracks = []
  audioSubscribed = false
  videoSubscribed = false
  selectedAudioTrack = null
  selectedVideoTrack = null
  audioPlaybackStarted = false
  moqtClient.clearSubgroupObjectHandlers()
  resetStatsSamples()
  updateCommandAlias(commandTrackAlias)
  updateCatalogAlias(catalogTrackAlias)
  renderCatalogList()
  updateSelectedAudioTrackLabel(selectedAudioTrack)
  updateSelectedVideoTrackLabel(selectedVideoTrack)
  updateStatus('disconnected', false)
}

const connectBtn = document.getElementById('connect-btn') as HTMLButtonElement | null
const disconnectBtn = document.getElementById('disconnect-btn') as HTMLButtonElement | null
const catalogSubscribeBtn = document.getElementById('catalog-subscribe-btn') as HTMLButtonElement | null
const audioSubscribeBtn = document.getElementById('audio-subscribe-btn') as HTMLButtonElement | null
const videoSubscribeBtn = document.getElementById('video-subscribe-btn') as HTMLButtonElement | null
const settingsBtn = document.getElementById('settings-btn') as HTMLButtonElement | null
const settingsCloseBtn = document.getElementById('settings-close-btn') as HTMLButtonElement | null
const settingsSaveBtn = document.getElementById('settings-save-btn') as HTMLButtonElement | null
const settingsModal = document.getElementById('settings-modal')

connectBtn?.addEventListener('click', () => {
  connect().catch((err) => {
    console.error(err)
    updateStatus('connection failed', false)
  })
})

disconnectBtn?.addEventListener('click', () => {
  disconnect().catch((err) => {
    console.error(err)
  })
})

catalogSubscribeBtn?.addEventListener('click', () => {
  subscribeCatalog().catch((err) => {
    console.error(err)
    updateStatus('catalog subscribe failed', false)
  })
})

videoSubscribeBtn?.addEventListener('click', () => {
  subscribeSelectedVideo().catch((err) => {
    console.error(err)
    updateStatus('video subscribe failed', false)
  })
})

audioSubscribeBtn?.addEventListener('click', () => {
  subscribeSelectedAudio().catch((err) => {
    console.error(err)
    updateStatus('audio subscribe failed', false)
  })
})

settingsBtn?.addEventListener('click', () => {
  openSettingsModal()
})

statsBtn?.addEventListener('click', () => {
  openStatsModal()
})

settingsCloseBtn?.addEventListener('click', () => {
  closeSettingsModal()
})

statsCloseBtn?.addEventListener('click', () => {
  closeStatsModal()
})

settingsSaveBtn?.addEventListener('click', () => {
  try {
    applyVideoDecoderSettings()
    closeSettingsModal()
    updateStatus('settings saved', true)
  } catch (error) {
    console.error('[onvif][settings] failed to apply settings', error)
    updateStatus('settings invalid', false)
  }
})

settingsModal?.addEventListener('click', (event) => {
  if (event.target === settingsModal) {
    closeSettingsModal()
  }
})

statsModal?.addEventListener('click', (event) => {
  if (event.target === statsModal) {
    closeStatsModal()
  }
})

document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && statsModal?.classList.contains('open')) {
    closeStatsModal()
    return
  }
  if (event.key === 'Escape' && settingsModal?.classList.contains('open')) {
    closeSettingsModal()
  }
})

buildCommandUI()
setupUrlPresets()
renderCatalogList()
resetStatsSamples()
