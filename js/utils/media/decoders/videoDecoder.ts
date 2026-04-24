import { VideoJitterBuffer } from '../videoJitterBuffer'
import type { JitterBufferSubgroupObject, SubgroupObjectWithLoc, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { createBitrateLogger } from '../bitrate'
import { tryDeserializeChunk, type ChunkMetadata } from '../chunk'
import { latencyMsFromCaptureMicros, monotonicUnixMicros } from '../clock'
import { bytesToBase64, readLocHeader } from '../loc'

const bitrateLogger = createBitrateLogger((kbps) => {
  postTelemetry({ type: 'bitrate', kbps })
})

type VideoDecoderHardwareAcceleration = 'prefer-hardware' | 'prefer-software'
const DEFAULT_VIDEO_DECODER_HARDWARE_ACCELERATION: VideoDecoderHardwareAcceleration = 'prefer-software'
let telemetryEnabled = true
let bypassJitterBuffer = false

function postTelemetry(message: unknown): void {
  if (!telemetryEnabled) {
    return
  }
  self.postMessage(message)
}

let videoDecoder: VideoDecoder | undefined
let currentDecoderHardwareAcceleration: VideoDecoderHardwareAcceleration = DEFAULT_VIDEO_DECODER_HARDWARE_ACCELERATION
let waitingForKeyFrame = true
let framesSinceLastKeyFrame: number | null = null
type LastDecodingObjectInfo = {
  groupId: string
  objectId: string
  chunkType: string
  codec?: string
}
let lastDecodingObject: LastDecodingObjectInfo | null = null

type DecodingPhase = 'submit' | 'output' | 'error'

function postDecodingObject(
  phase: DecodingPhase,
  groupId: string,
  objectId: string,
  chunkType: string,
  codec?: string
): void {
  postTelemetry({
    type: 'decodingObject',
    media: 'video',
    phase,
    groupId,
    objectId,
    chunkType,
    codec
  })
}

function postBufferedObject(groupId: bigint, objectId: bigint): void {
  postTelemetry({
    type: 'bufferedObject',
    media: 'video',
    groupId,
    objectId
  })
}

async function initializeVideoDecoder(config: VideoDecoderConfig) {
  function sendVideoFrameMessage(frame: VideoFrame): void {
    enqueueOutputFrame(frame)
  }

  const init: VideoDecoderInit = {
    output: sendVideoFrameMessage,
    error: (e: any) => {
      const last = lastDecodingObject ?? { groupId: '?', objectId: '?', chunkType: '?', codec: undefined }
      postDecodingObject('error', last.groupId, last.objectId, last.chunkType, last.codec)
      console.warn('[videoDecoder] decoder error', e)
      videoDecoder = undefined
      cachedVideoConfig = null
      waitingForKeyFrame = true
      resetPlayoutTiming('error')
    }
  }
  try {
    const supported = await VideoDecoder.isConfigSupported(config)
    if (!supported.supported) {
      console.error('[videoDecoder] unsupported decoder config', config)
      self.postMessage({ type: 'configError', media: 'video', reason: 'unsupported', config })
      return undefined
    }
  } catch (error) {
    console.error('[videoDecoder] failed to check decoder config support', error, config)
    self.postMessage({ type: 'configError', media: 'video', reason: 'unsupported', config })
    return undefined
  }
  const decoder = new VideoDecoder(init)
  try {
    decoder.configure(config)
  } catch (error) {
    console.error('[videoDecoder] configure failed', error, config)
    self.postMessage({ type: 'configError', media: 'video', reason: 'configure_failed', config })
    try {
      decoder.close()
    } catch {
      // ignore close failure on configure error path
    }
    return undefined
  }
  return decoder
}

type VideoJitterBufferConfig = {
  pacing?: VideoPacingConfigInput
  decoderHardwareAcceleration?: VideoDecoderHardwareAcceleration
}

const POP_INTERVAL_MS = 33
const MISSING_META_WARN_SUPPRESS_AFTER_RESET_MS = 3000
const MAX_FRAMES_PER_DRAIN = 2

type VideoPacingConfig = {
  preset: VideoPacingPreset
  pipeline: VideoPacingPipeline
  fallbackIntervalMs: number
  lateThresholdMs: number
  maxWaitMs: number
  defaultFrameIntervalUs: number
  minFrameIntervalUs: number
  maxFrameIntervalUs: number
  reportIntervalMs: number
  targetLatencyMs: number
  decodedBufferMax: number
}

type VideoPacingPreset = 'disabled' | 'onvif' | 'call'
type VideoPacingPipeline = 'buffer-pacing-decode'
type VideoPacingConfigInput = Partial<VideoPacingConfig>

function createPacingPresetConfig(preset: VideoPacingPreset): VideoPacingConfig {
  if (preset === 'disabled') {
    return {
      ...createPacingPresetConfig('onvif'),
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
      defaultFrameIntervalUs: 33_333,
      minFrameIntervalUs: 8_000,
      maxFrameIntervalUs: 200_000,
      reportIntervalMs: 500,
      targetLatencyMs: 250,
      decodedBufferMax: 36
    }
  }
  return {
    ...createPacingPresetConfig('onvif'),
    preset: 'call',
    pipeline: 'buffer-pacing-decode',
    targetLatencyMs: 250
  }
}

const DEFAULT_PACING_CONFIG: VideoPacingConfig = createPacingPresetConfig('onvif')

type NormalizedJitterConfig = {
  minDelayMs: number
}

const DEFAULT_JITTER_CONFIG: NormalizedJitterConfig = {
  minDelayMs: 35
}

let jitterBuffer = createJitterBuffer()
let currentJitterConfig: NormalizedJitterConfig = { ...DEFAULT_JITTER_CONFIG }
let currentPacingConfig: VideoPacingConfig = { ...DEFAULT_PACING_CONFIG }
let popTimer: number | null = null
let encodedPacingTimer: number | null = null
let pendingEncodedEntry: JitterBufferEntryForPacing | null = null
let pendingEncodedTimer: number | null = null
let encodedPacingBuffer: JitterBufferEntryForPacing[] = []
let playoutBaseMediaTs: number | null = null
let playoutBaseWallMs: number | null = null
let pacingFrameIntervalUs = DEFAULT_PACING_CONFIG.defaultFrameIntervalUs
let pacingDelayMs = DEFAULT_JITTER_CONFIG.minDelayMs
let pacingLastReason: string | null = null
let pacingLastReportMs = 0
let decodedFrameBuffer: DecodedFrameEntry[] = []
let decodedDrainTimer: number | null = null
let outputPendingMeta: Map<number, OutputMeta[]> = new Map()
let receivedFrameTimes: Map<string, number> = new Map()
let lastOutputMetaResetAtMs = Number.NEGATIVE_INFINITY

type OutputMeta = {
  groupId: bigint
  objectId: bigint
  chunkType: string
  timestampUs: number
  receivedMs?: number
  captureTimestampMicros?: number
}

type DecodedFrameEntry = {
  groupId: bigint
  objectId: bigint
  chunkType: string
  timestampUs: number
  frame: VideoFrame
  decodeDoneMs: number
  receivedMs: number | null
  captureTimestampMicros?: number
}

type JitterBufferEntryForPacing = NonNullable<ReturnType<VideoJitterBuffer['popWithMetadata']>>

function normalizeJitterConfig(config?: VideoJitterBufferConfig): NormalizedJitterConfig {
  void config
  return { ...DEFAULT_JITTER_CONFIG }
}

function clampNumber(value: unknown, fallback: number, min: number, max = Number.POSITIVE_INFINITY): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return fallback
  }
  return Math.min(max, Math.max(min, value))
}

function normalizePacingConfig(config?: VideoPacingConfigInput): VideoPacingConfig {
  const preset: VideoPacingPreset =
    config?.preset === 'disabled' || config?.preset === 'call' || config?.preset === 'onvif'
      ? config.preset
      : currentPacingConfig.preset === 'disabled' ||
          currentPacingConfig.preset === 'call' ||
          currentPacingConfig.preset === 'onvif'
        ? currentPacingConfig.preset
        : 'onvif'
  const merged: VideoPacingConfigInput = {
    ...currentPacingConfig,
    ...createPacingPresetConfig(preset),
    ...(config ?? {}),
    preset
  }

  const minFrameIntervalUs = Math.round(
    clampNumber(merged.minFrameIntervalUs, currentPacingConfig.minFrameIntervalUs, 1_000, 1_000_000)
  )
  const maxFrameIntervalUs = Math.round(
    clampNumber(merged.maxFrameIntervalUs, currentPacingConfig.maxFrameIntervalUs, minFrameIntervalUs, 2_000_000)
  )
  const targetLatencyMs = clampNumber(merged.targetLatencyMs, currentPacingConfig.targetLatencyMs, 0, 30_000)

  return {
    preset,
    pipeline: 'buffer-pacing-decode',
    fallbackIntervalMs: Math.round(
      clampNumber(merged.fallbackIntervalMs, currentPacingConfig.fallbackIntervalMs, 1, 1000)
    ),
    lateThresholdMs: clampNumber(merged.lateThresholdMs, currentPacingConfig.lateThresholdMs, 0, 10_000),
    maxWaitMs: clampNumber(merged.maxWaitMs, currentPacingConfig.maxWaitMs, 1, 10_000),
    defaultFrameIntervalUs: Math.round(
      clampNumber(
        merged.defaultFrameIntervalUs,
        currentPacingConfig.defaultFrameIntervalUs,
        minFrameIntervalUs,
        maxFrameIntervalUs
      )
    ),
    minFrameIntervalUs,
    maxFrameIntervalUs,
    reportIntervalMs: Math.round(
      clampNumber(merged.reportIntervalMs, currentPacingConfig.reportIntervalMs, 50, 10_000)
    ),
    targetLatencyMs,
    decodedBufferMax: Math.round(clampNumber(merged.decodedBufferMax, currentPacingConfig.decodedBufferMax, 1, 1000))
  }
}

function createJitterBuffer(config?: VideoJitterBufferConfig): VideoJitterBuffer {
  void config
  return new VideoJitterBuffer(9000)
}

function updatePacingConfig(config?: VideoPacingConfigInput): void {
  if (!config) {
    return
  }
  currentPacingConfig = normalizePacingConfig(config)
  pacingFrameIntervalUs = Math.round(
    clampNumber(
      pacingFrameIntervalUs,
      currentPacingConfig.defaultFrameIntervalUs,
      currentPacingConfig.minFrameIntervalUs,
      currentPacingConfig.maxFrameIntervalUs
    )
  )
}

function updateJitterBuffer(config: VideoJitterBufferConfig): void {
  const nextDecoderHardwareAcceleration =
    config.decoderHardwareAcceleration === 'prefer-software' || config.decoderHardwareAcceleration === 'prefer-hardware'
      ? config.decoderHardwareAcceleration
      : currentDecoderHardwareAcceleration
  const decoderHardwareAccelerationChanged = nextDecoderHardwareAcceleration !== currentDecoderHardwareAcceleration
  currentDecoderHardwareAcceleration = nextDecoderHardwareAcceleration
  currentJitterConfig = normalizeJitterConfig({ ...currentJitterConfig, ...config })
  updatePacingConfig(config.pacing)
  jitterBuffer = createJitterBuffer(currentJitterConfig)
  if (decoderHardwareAccelerationChanged) {
    if (videoDecoder && videoDecoder.state !== 'closed') {
      try {
        videoDecoder.close()
      } catch {
        // ignore close failure during decoder config switch
      }
    }
    videoDecoder = undefined
    cachedVideoConfig = null
    waitingForKeyFrame = true
  }
  pacingDelayMs = currentJitterConfig.minDelayMs
  resetPlayoutTiming('config')
}

type CachedVideoConfig = { codec: string; descriptionBase64?: string; avcFormat?: 'annexb' | 'avc' }

let cachedVideoConfig: CachedVideoConfig | null = null
let catalogCodec: string | null = null
let catalogFramerate: number | null = null
let missingCatalogCodecWarned = false
let directDecodeQueue: Promise<void> = Promise.resolve()
const directLastObjectIds = new Map<string, bigint>()

function summarizeDescriptionBase64(descriptionBase64?: string): {
  length: number
  preview?: string
} {
  if (!descriptionBase64) {
    return { length: 0 }
  }
  return {
    length: descriptionBase64.length,
    preview: descriptionBase64.slice(0, 48)
  }
}

function summarizeUint8Array(bytes?: Uint8Array): {
  length: number
  preview?: number[]
} {
  if (!(bytes instanceof Uint8Array)) {
    return { length: 0 }
  }
  return {
    length: bytes.byteLength,
    preview: Array.from(bytes.slice(0, 16))
  }
}

function resolvePlayoutDelayMs(): number {
  return pacingDelayMs
}

function resetPlayoutTiming(reason: 'config' | 'error' | 'jump'): void {
  playoutBaseMediaTs = null
  playoutBaseWallMs = null
  pacingDelayMs = currentJitterConfig.minDelayMs
  clearOutputQueue()
  reportPacingStatus(reason)
}

function clearOutputQueue(): void {
  if (encodedPacingTimer !== null) {
    clearTimeout(encodedPacingTimer)
    encodedPacingTimer = null
  }
  if (pendingEncodedTimer !== null) {
    clearTimeout(pendingEncodedTimer)
    pendingEncodedTimer = null
  }
  if (decodedDrainTimer !== null) {
    clearTimeout(decodedDrainTimer)
    decodedDrainTimer = null
  }
  for (const entry of decodedFrameBuffer) {
    entry.frame.close()
  }
  decodedFrameBuffer = []
  encodedPacingBuffer = []
  pendingEncodedEntry = null
  outputPendingMeta.clear()
  lastOutputMetaResetAtMs = performance.now()
  receivedFrameTimes.clear()
  lastDecodingObject = null
}

function schedulePop(delayMs: number): void {
  if (popTimer !== null) {
    clearTimeout(popTimer)
  }
  popTimer = setTimeout(pumpJitterBuffer, delayMs)
}

function getBufferedFrameCount(): number {
  return decodedFrameBuffer.length + encodedPacingBuffer.length + (pendingEncodedEntry ? 1 : 0)
}

function resolveCaptureTargetWaitMs(captureTimestampMicros?: number): number | null {
  if (currentPacingConfig.preset === 'disabled') {
    return null
  }
  if (!(typeof captureTimestampMicros === 'number' && Number.isFinite(captureTimestampMicros))) {
    return null
  }
  if (!(currentPacingConfig.targetLatencyMs > 0)) {
    return null
  }
  const nowWallMs = monotonicUnixMicros() / 1000
  pacingDelayMs = Math.max(currentJitterConfig.minDelayMs, currentPacingConfig.targetLatencyMs)
  return captureTimestampMicros / 1000 + pacingDelayMs - nowWallMs
}

function resolveTimestampTimelineWaitMs(timestampUs: number): number | null {
  if (!Number.isFinite(timestampUs)) {
    return null
  }
  pacingDelayMs =
    currentPacingConfig.targetLatencyMs > 0
      ? Math.max(currentJitterConfig.minDelayMs, currentPacingConfig.targetLatencyMs)
      : currentJitterConfig.minDelayMs

  const nowMs = performance.now()
  if (playoutBaseMediaTs === null || playoutBaseWallMs === null || timestampUs < playoutBaseMediaTs) {
    playoutBaseMediaTs = timestampUs
    playoutBaseWallMs = nowMs + resolvePlayoutDelayMs()
  }
  const renderAtMs = playoutBaseWallMs + (timestampUs - playoutBaseMediaTs) / 1000
  return renderAtMs - nowMs
}

function scheduleEncodedPacing(delayMs: number): void {
  if (encodedPacingTimer !== null) {
    return
  }
  encodedPacingTimer = setTimeout(() => {
    encodedPacingTimer = null
    pumpEncodedPacingDecode()
  }, delayMs)
}

function enqueueEncodedPacing(entry: JitterBufferEntryForPacing): void {
  encodedPacingBuffer.push(entry)
  scheduleEncodedPacing(0)
}

function pumpEncodedPacingDecode(): void {
  if (pendingEncodedEntry) {
    return
  }
  const entry = encodedPacingBuffer[0]
  if (!entry) {
    return
  }
  const waitMs = resolveCaptureTargetWaitMs(entry.captureTimestampMicros)
  if (typeof waitMs === 'number' && waitMs > 0) {
    const boundedWaitMs = Math.min(waitMs, currentPacingConfig.maxWaitMs)
    pendingEncodedEntry = entry
    pendingEncodedTimer = setTimeout(() => {
      pendingEncodedTimer = null
      pendingEncodedEntry = null
      reportPacingStatus('wait', boundedWaitMs)
      pumpEncodedPacingDecode()
    }, boundedWaitMs)
    return
  }
  const lateByMs = typeof waitMs === 'number' && waitMs < 0 ? -waitMs : 0
  const next = encodedPacingBuffer.shift()
  if (next) {
    if (lateByMs > currentPacingConfig.lateThresholdMs) {
      pacingLastReason = 'late'
    }
    decodeBufferedEntryNow(next)
  }
  reportPacingStatus('decode', lateByMs)
  scheduleEncodedPacing(0)
}

function pumpJitterBuffer(): void {
  if (decodedFrameBuffer.length >= currentPacingConfig.decodedBufferMax) {
    schedulePop(POP_INTERVAL_MS)
    return
  }
  const entry = jitterBuffer.popHolding()
  if (!entry) {
    schedulePop(POP_INTERVAL_MS)
    return
  }
  postJitterBufferActivity('pop')
  if (entry.isEndOfGroup) {
    schedulePop(0)
    return
  }
  handlePacedDecode(entry)
}

function handlePacedDecode(entry: NonNullable<ReturnType<VideoJitterBuffer['popWithMetadata']>>): void {
  if (currentPacingConfig.preset !== 'disabled') {
    enqueueEncodedPacing(entry)
    schedulePop(0)
    return
  }
  decodeBufferedEntryNow(entry)
}

function decodeBufferedEntryNow(entry: NonNullable<ReturnType<VideoJitterBuffer['popWithMetadata']>>): void {
  const decoded = entry.object.cachedChunk
  if (waitingForKeyFrame && decoded.metadata.type !== 'key') {
    schedulePop(0)
    return
  }
  const decodeQueueSize = videoDecoder?.decodeQueueSize ?? 0
  const objectKey = `${entry.groupId.toString()}:${entry.object.objectId.toString()}`
  registerOutputMeta(decoded.metadata.timestamp, {
    groupId: entry.groupId,
    objectId: entry.object.objectId,
    chunkType: decoded.metadata.type,
    timestampUs: decoded.metadata.timestamp,
    receivedMs: receivedFrameTimes.get(objectKey),
    captureTimestampMicros: entry.captureTimestampMicros
  })
  receivedFrameTimes.delete(objectKey)
  decode(entry.groupId, entry.object, entry.captureTimestampMicros)
  schedulePop(decodeQueueSize > 12 ? POP_INTERVAL_MS : 0)
}

function registerOutputMeta(timestampUs: number, meta: OutputMeta): void {
  const key = Math.round(timestampUs)
  const list = outputPendingMeta.get(key) ?? []
  list.push({ ...meta, timestampUs: key })
  outputPendingMeta.set(key, list)
}

function dequeueOutputMeta(timestampUs: number): OutputMeta | null {
  const key = Math.round(timestampUs)
  const list = outputPendingMeta.get(key)
  if (!list || list.length === 0) {
    return null
  }
  const meta = list.shift() ?? null
  if (list.length === 0) {
    outputPendingMeta.delete(key)
  } else {
    outputPendingMeta.set(key, list)
  }
  return meta
}

function enqueueOutputFrame(frame: VideoFrame): void {
  const timestampUs = Math.round(Number(frame.timestamp))
  const meta = Number.isFinite(timestampUs) ? dequeueOutputMeta(timestampUs) : null
  if (!meta) {
    const nowMs = performance.now()
    const recentlyReset = nowMs - lastOutputMetaResetAtMs <= MISSING_META_WARN_SUPPRESS_AFTER_RESET_MS
    if (recentlyReset) {
      frame.close()
      return
    }
    console.warn('[videoDecoder][output] missing meta for decoded frame', {
      timestampUs: Number.isFinite(timestampUs) ? timestampUs : null,
      bufferedFrames: getBufferedFrameCount(),
      targetFrames: resolveTargetFrames()
    })
    frame.close()
    return
  }
  const receivedKey = `${meta.groupId.toString()}:${meta.objectId.toString()}`
  const receivedMs = typeof meta.receivedMs === 'number' ? meta.receivedMs : receivedFrameTimes.get(receivedKey) ?? null
  if (receivedFrameTimes.has(receivedKey)) {
    receivedFrameTimes.delete(receivedKey)
  }
  const decodeDoneMs = performance.now()
  postDecodingObject('output', meta.groupId.toString(), meta.objectId.toString(), meta.chunkType)
  pushDecodedFrame({
    groupId: meta.groupId,
    objectId: meta.objectId,
    chunkType: meta.chunkType,
    timestampUs: meta.timestampUs,
    frame,
    decodeDoneMs,
    receivedMs,
    captureTimestampMicros: meta.captureTimestampMicros
  })
  drainDecodedOutput()
}

function pushDecodedFrame(entry: DecodedFrameEntry): void {
  const insertIndex = decodedFrameBuffer.findIndex(
    (item) => item.groupId > entry.groupId || (item.groupId === entry.groupId && item.objectId > entry.objectId)
  )
  if (insertIndex === -1) {
    decodedFrameBuffer.push(entry)
  } else {
    decodedFrameBuffer.splice(insertIndex, 0, entry)
  }
  if (decodedFrameBuffer.length > currentPacingConfig.decodedBufferMax) {
    const dropped = decodedFrameBuffer.shift()
    dropped?.frame.close()
    console.warn('[videoDecoder][output] decoded buffer overflow, dropping frame', {
      bufferedFrames: getBufferedFrameCount(),
      targetFrames: resolveTargetFrames(),
      bufferSize: decodedFrameBuffer.length
    })
  }
}

function drainDecodedOutput(): void {
  let drained = 0
  while (decodedFrameBuffer.length > 0 && drained < MAX_FRAMES_PER_DRAIN) {
    emitDecodedFrame()
    drained += 1
  }
  if (decodedFrameBuffer.length === 0 || decodedDrainTimer !== null) {
    return
  }
  decodedDrainTimer = setTimeout(() => {
    decodedDrainTimer = null
    drainDecodedOutput()
  }, 0)
}

function emitDecodedFrame(): void {
  const entry = decodedFrameBuffer.shift()
  if (!entry) {
    return
  }
  const renderMs = performance.now()
  const receiveToDecodeMs = entry.receivedMs !== null ? Math.round(entry.decodeDoneMs - entry.receivedMs) : null
  const receiveToRenderMs = entry.receivedMs !== null ? Math.round(renderMs - entry.receivedMs) : null
  reportLatency(entry.captureTimestampMicros)
  postTelemetry({
    type: 'timing',
    media: 'video',
    receiveToDecodeMs,
    receiveToRenderMs
  })
  self.postMessage(
    {
      type: 'frame',
      frame: entry.frame,
      width: entry.frame.displayWidth,
      height: entry.frame.displayHeight
    },
    [entry.frame]
  )
}

schedulePop(currentPacingConfig.fallbackIntervalMs)

type DecoderControlMessage =
  | { type: 'config'; config: VideoJitterBufferConfig & { telemetryEnabled?: boolean; bypassJitterBuffer?: boolean } }
  | { type: 'catalog'; codec?: string; framerate?: number }
type WorkerMessage = SubgroupWorkerMessage | DecoderControlMessage

function isConfigMessage(
  message: WorkerMessage
): message is {
  type: 'config'
  config: VideoJitterBufferConfig & { telemetryEnabled?: boolean; bypassJitterBuffer?: boolean }
} {
  return (message as { type?: string }).type === 'config'
}

function isCatalogMessage(message: WorkerMessage): message is { type: 'catalog'; codec?: string; framerate?: number } {
  return (message as { type?: string }).type === 'catalog'
}

self.onmessage = async (event: MessageEvent<WorkerMessage>) => {
  if (isConfigMessage(event.data)) {
    telemetryEnabled = event.data.config.telemetryEnabled ?? telemetryEnabled
    bypassJitterBuffer = event.data.config.bypassJitterBuffer ?? bypassJitterBuffer
    updateJitterBuffer(event.data.config)
    return
  }
  if (isCatalogMessage(event.data)) {
    applyCatalogInfo(event.data.codec, event.data.framerate)
    return
  }
  const subgroupStreamObject: SubgroupObjectWithLoc = {
    subgroupId: event.data.subgroupStreamObject.subgroupId,
    objectIdDelta: event.data.subgroupStreamObject.objectIdDelta,
    objectPayloadLength: event.data.subgroupStreamObject.objectPayloadLength,
    objectPayload: event.data.subgroupStreamObject.objectPayload,
    objectStatus: event.data.subgroupStreamObject.objectStatus,
    locHeader: event.data.subgroupStreamObject.locHeader
  }
  bitrateLogger.addBytes(subgroupStreamObject.objectPayloadLength)

  if (bypassJitterBuffer) {
    enqueueDirectDecode(event.data.groupId, subgroupStreamObject)
    return
  }

  const objectId = jitterBuffer.push(event.data.groupId, subgroupStreamObject, (latencyMs) =>
    postReceiveLatency(latencyMs)
  )
  if (objectId !== null) {
    const receiveKey = `${event.data.groupId.toString()}:${objectId.toString()}`
    postJitterBufferActivity('push')
    postBufferedObject(event.data.groupId, objectId)
    receivedFrameTimes.set(receiveKey, performance.now())
    schedulePop(0)
  }
}

function enqueueDirectDecode(groupId: bigint, subgroupStreamObject: SubgroupObjectWithLoc): void {
  directDecodeQueue = directDecodeQueue
    .then(async () => {
      const entry = materializeDirectObject(groupId, subgroupStreamObject, (latencyMs) => postReceiveLatency(latencyMs))
      if (!entry) {
        return
      }
      postBufferedObject(groupId, entry.object.objectId)
      const decoded = entry.object.cachedChunk
      if (waitingForKeyFrame && decoded.metadata.type !== 'key') {
        return
      }
      registerOutputMeta(decoded.metadata.timestamp, {
        groupId,
        objectId: entry.object.objectId,
        chunkType: decoded.metadata.type,
        timestampUs: decoded.metadata.timestamp,
        receivedMs: performance.now(),
        captureTimestampMicros: entry.captureTimestampMicros
      })
      await decode(groupId, entry.object, entry.captureTimestampMicros)
    })
    .catch((error) => {
      console.error('[videoDecoder] direct decode failed', error)
    })
}

function materializeDirectObject(
  groupId: bigint,
  object: SubgroupObjectWithLoc,
  onReceiveLatency?: (latencyMs: number) => void
): { object: JitterBufferSubgroupObject; captureTimestampMicros?: number } | null {
  const subgroupId = normalizeSubgroupId(object.subgroupId)
  const objectId = assignDirectObjectId(groupId, subgroupId, object.objectIdDelta)
  if (isTerminalStatus(object.objectStatus)) {
    directLastObjectIds.delete(makeSubgroupKey(groupId, subgroupId))
  }
  if (!object.objectPayloadLength) {
    return null
  }

  const locMetadata = readLocHeader(object.locHeader)
  const captureTimestampMicros = getCaptureTimestampMicros(locMetadata.captureTimestampMicros)
  const parsed = tryDeserializeChunk(object.objectPayload) ?? buildChunkFromLoc(object, objectId)
  if (!parsed) {
    return null
  }
  if (!parsed.metadata.descriptionBase64 && locMetadata.videoConfig) {
    parsed.metadata.descriptionBase64 = bytesToBase64(locMetadata.videoConfig)
  }

  if (typeof captureTimestampMicros === 'number') {
    onReceiveLatency?.(latencyMsFromCaptureMicros(captureTimestampMicros))
  }

  return {
    captureTimestampMicros,
    object: {
      ...object,
      objectId,
      cachedChunk: parsed,
      remotePTS: parsed.metadata.timestamp,
      localPTS: performance.timeOrigin + performance.now()
    }
  }
}

function assignDirectObjectId(groupId: bigint, subgroupId: bigint, objectIdDelta: bigint): bigint {
  const key = makeSubgroupKey(groupId, subgroupId)
  const previousObjectId = directLastObjectIds.get(key)
  const objectId =
    previousObjectId === undefined ? (objectIdDelta > 0n ? objectIdDelta - 1n : 0n) : previousObjectId + objectIdDelta
  directLastObjectIds.set(key, objectId)
  return objectId
}

function normalizeSubgroupId(subgroupId: bigint | undefined): bigint {
  return subgroupId ?? 0n
}

function makeSubgroupKey(groupId: bigint, subgroupId: bigint): string {
  return `${groupId.toString()}:${subgroupId.toString()}`
}

function isTerminalStatus(status: number | undefined): boolean {
  return status === 3 || status === 4
}

function buildChunkFromLoc(
  object: SubgroupObjectWithLoc,
  objectId: bigint
): { metadata: ChunkMetadata; data: Uint8Array } | null {
  const loc = readLocHeader(object.locHeader)
  const captureMicros = getCaptureTimestampMicros(loc.captureTimestampMicros)
  const metadata: ChunkMetadata = {
    type: objectId === 0n ? 'key' : 'delta',
    timestamp: typeof captureMicros === 'number' ? captureMicros : 0,
    duration: null,
    descriptionBase64: loc.videoConfig ? bytesToBase64(loc.videoConfig) : undefined
  }
  return { metadata, data: object.objectPayload }
}

function getCaptureTimestampMicros(value: number | undefined): number | undefined {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < 0) {
    return undefined
  }
  return value
}

async function decode(
  groupId: bigint,
  subgroupStreamObject: JitterBufferSubgroupObject,
  captureTimestampMicros?: number
) {
  const decoded = subgroupStreamObject.cachedChunk
  trackKeyframeInterval(decoded.metadata.type)

  const resolvedConfig = resolveVideoConfig(decoded.metadata)
  if (!resolvedConfig) {
    console.info('[videoDecoder] decode config unresolved', {
      groupId: groupId.toString(),
      objectId: subgroupStreamObject.objectId.toString(),
      chunkType: decoded.metadata.type,
      metadataCodec: decoded.metadata.codec,
      metadataAvcFormat: decoded.metadata.avcFormat,
      metadataDescription: summarizeDescriptionBase64(decoded.metadata.descriptionBase64),
      catalogCodec,
      cachedVideoConfig
    })
    return
  }
  const desiredConfig = buildVideoDecoderConfig(resolvedConfig)
  const desiredDescription = desiredConfig.description instanceof Uint8Array ? desiredConfig.description : undefined
  console.info('[videoDecoder] decode config', {
    groupId: groupId.toString(),
    objectId: subgroupStreamObject.objectId.toString(),
    chunkType: decoded.metadata.type,
    waitingForKeyFrame,
    metadata: {
      codec: decoded.metadata.codec,
      avcFormat: decoded.metadata.avcFormat,
      timestamp: decoded.metadata.timestamp,
      duration: decoded.metadata.duration,
      description: summarizeDescriptionBase64(decoded.metadata.descriptionBase64)
    },
    resolvedConfig: {
      codec: resolvedConfig.codec,
      avcFormat: resolvedConfig.avcFormat,
      description: summarizeDescriptionBase64(resolvedConfig.descriptionBase64)
    },
    desiredConfig: {
      codec: desiredConfig.codec,
      avcFormat:
        desiredConfig.avc?.format === 'avc' || desiredConfig.avc?.format === 'annexb'
          ? desiredConfig.avc.format
          : undefined,
      description: summarizeUint8Array(desiredDescription),
      hardwareAcceleration: desiredConfig.hardwareAcceleration
    },
    cachedVideoConfig
  })

  const encodedVideoChunk = new EncodedVideoChunk({
    type: decoded.metadata.type as EncodedVideoChunkType,
    timestamp: decoded.metadata.timestamp,
    duration: decoded.metadata.duration ?? undefined,
    data: decoded.data
  })

  if (!videoDecoder || videoDecoder.state === 'closed') {
    videoDecoder = await initializeVideoDecoder(desiredConfig)
    if (!videoDecoder) {
      return
    }
    cachedVideoConfig = resolvedConfig
    postDecoderConfig(resolvedConfig, desiredConfig)
    waitingForKeyFrame = true
    resetPlayoutTiming('config')
    if (decoded.metadata.type !== 'key') {
      return
    }
  }

  if (waitingForKeyFrame && decoded.metadata.type !== 'key') {
    return
  }

  try {
    lastDecodingObject = {
      groupId: groupId.toString(),
      objectId: subgroupStreamObject.objectId.toString(),
      chunkType: decoded.metadata.type,
      codec: resolvedConfig.codec
    }
    postDecodingObject(
      'submit',
      groupId.toString(),
      subgroupStreamObject.objectId.toString(),
      decoded.metadata.type,
      resolvedConfig.codec
    )
    await videoDecoder.decode(encodedVideoChunk)
    if (decoded.metadata.type === 'key') {
      waitingForKeyFrame = false
    }
  } catch (error) {
    if (isKeyFrameRequiredError(error)) {
      waitingForKeyFrame = true
      resetPlayoutTiming('error')
      console.warn('[videoDecoder] decode requires key frame; waiting for next key frame', {
        groupId: groupId.toString(),
        objectId: subgroupStreamObject.objectId.toString(),
        chunkType: decoded.metadata.type
      })
      return
    }
    throw error
  }
}

function reportPacingStatus(action: 'decode' | 'wait' | 'config' | 'error' | 'jump', detailMs?: number) {
  const nowMs = performance.now()
  if (nowMs - pacingLastReportMs < currentPacingConfig.reportIntervalMs) {
    return
  }
  pacingLastReportMs = nowMs
  const bufferedFrames = getBufferedFrameCount()
  postTelemetry({
    type: 'pacing',
    media: 'video',
    intervalMs: pacingFrameIntervalUs / 1000,
    effectiveIntervalMs: pacingFrameIntervalUs / 1000,
    bufferedFrames,
    decodeQueueSize: videoDecoder?.decodeQueueSize ?? 0,
    targetFrames: 1,
    lastReason: pacingLastReason ?? undefined,
    action,
    detailMs
  })
}

function resolveTargetFrames(): number {
  return 1
}

function trackKeyframeInterval(chunkType: string): void {
  if (framesSinceLastKeyFrame !== null) {
    framesSinceLastKeyFrame += 1
  }
  if (chunkType !== 'key') {
    return
  }
  if (typeof framesSinceLastKeyFrame === 'number' && framesSinceLastKeyFrame > 0) {
    postTelemetry({ type: 'keyframeInterval', media: 'video', frames: framesSinceLastKeyFrame })
  }
  framesSinceLastKeyFrame = 0
}

function reportLatency(captureTimestampMicros: number | undefined) {
  if (typeof captureTimestampMicros !== 'number') {
    return
  }
  const latency = latencyMsFromCaptureMicros(captureTimestampMicros)
  if (latency < 0) {
    return
  }
  postRenderingLatency(latency)
}

function isKeyFrameRequiredError(error: unknown): boolean {
  if (!(error instanceof DOMException)) {
    return false
  }
  return error.name === 'DataError' && error.message.includes('A key frame is required')
}

function postReceiveLatency(latencyMs: number) {
  if (latencyMs < 0) {
    return
  }
  postTelemetry({ type: 'receiveLatency', media: 'video', ms: latencyMs })
}

function postRenderingLatency(latencyMs: number) {
  if (latencyMs < 0) {
    return
  }
  postTelemetry({ type: 'renderingLatency', media: 'video', ms: latencyMs })
}

function postJitterBufferActivity(event: 'push' | 'pop') {
  postTelemetry({
    type: 'jitterBufferActivity',
    media: 'video',
    event,
    bufferedFrames: getBufferedFrameCount(),
    capacityFrames: jitterBuffer.getMaxBufferSize()
  })
}

function postDecoderConfig(resolvedConfig: CachedVideoConfig, desired: VideoDecoderConfig) {
  const descriptionLength = desired.description instanceof Uint8Array ? desired.description.byteLength : undefined
  const avcFormat =
    resolvedConfig.avcFormat ??
    (desired.avc?.format === 'avc' || desired.avc?.format === 'annexb' ? desired.avc.format : undefined)
  const hardwareAcceleration =
    desired.hardwareAcceleration === 'prefer-hardware' ||
    desired.hardwareAcceleration === 'prefer-software' ||
    desired.hardwareAcceleration === 'no-preference'
      ? desired.hardwareAcceleration
      : undefined
  postTelemetry({
    type: 'decoderConfig',
    codec: resolvedConfig.codec,
    descriptionLength,
    avcFormat,
    hardwareAcceleration,
    optimizeForLatency: desired.optimizeForLatency
  })
}

function applyCatalogInfo(codec?: string, framerate?: number): void {
  if (typeof framerate === 'number' && Number.isFinite(framerate) && framerate > 0) {
    if (catalogFramerate !== framerate) {
      catalogFramerate = framerate
      const intervalUs = Math.round(1_000_000 / framerate)
      pacingFrameIntervalUs = Math.min(
        currentPacingConfig.maxFrameIntervalUs,
        Math.max(currentPacingConfig.minFrameIntervalUs, intervalUs)
      )
      pacingLastReason = 'catalog-fps'
      resetPlayoutTiming('config')
    }
  }
  if (!codec) {
    return
  }
  if (catalogCodec === codec) {
    return
  }
  catalogCodec = codec
  if (cachedVideoConfig && cachedVideoConfig.codec !== codec) {
    console.warn('[videoDecoder] catalog codec changed after decoder initialization; ignoring new codec', {
      initializedCodec: cachedVideoConfig.codec,
      catalogCodec: codec
    })
  }
}

function resolveVideoConfig(metadata: ChunkMetadata): CachedVideoConfig | null {
  if (cachedVideoConfig) {
    return cachedVideoConfig
  }
  const codec = metadata.codec ?? catalogCodec ?? undefined
  if (!codec) {
    if (!missingCatalogCodecWarned) {
      missingCatalogCodecWarned = true
      console.warn('[videoDecoder] skip decode until catalog codec is available')
    }
    return null
  }
  missingCatalogCodecWarned = false
  return {
    codec,
    descriptionBase64: metadata.descriptionBase64,
    avcFormat: metadata.avcFormat ?? (codec.startsWith('avc') ? 'annexb' : undefined)
  }
}

function buildVideoDecoderConfig(resolved: CachedVideoConfig): VideoDecoderConfig {
  if (resolved.codec.startsWith('avc')) {
    const description = resolved.descriptionBase64 ? base64ToUint8Array(resolved.descriptionBase64) : undefined
    return {
      codec: resolved.codec,
      description,
      avc: {
        format: resolved.avcFormat ?? 'avc'
      },
      hardwareAcceleration: currentDecoderHardwareAcceleration as any
    } as any
  }

  return {
    codec: resolved.codec,
    description: resolved.descriptionBase64 ? base64ToUint8Array(resolved.descriptionBase64) : undefined,
    hardwareAcceleration: currentDecoderHardwareAcceleration as any
  } as VideoDecoderConfig
}

function base64ToUint8Array(base64: string): Uint8Array {
  const binaryString = atob(base64)
  const len = binaryString.length
  const bytes = new Uint8Array(len)
  for (let i = 0; i < len; i++) {
    bytes[i] = binaryString.charCodeAt(i)
  }
  return bytes
}
