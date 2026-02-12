import { VideoJitterBuffer, type VideoJitterBufferMode } from '../videoJitterBuffer'
import type { JitterBufferSubgroupObject, SubgroupObjectWithLoc, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { KEYFRAME_INTERVAL } from '../constants'
import { createBitrateLogger } from '../bitrate'
import type { ChunkMetadata } from '../chunk'

const bitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', kbps })
})

const KEYFRAME_INTERVAL_BIGINT = BigInt(KEYFRAME_INTERVAL)

const VIDEO_DECODER_CONFIG = {
  //codec: 'av01.0.08M.08',
  codec: 'avc1.640032',
  hardwareAcceleration: 'prefer-hardware' as any,
  scalabilityMode: 'L1T1'
}

let videoDecoder: VideoDecoder | undefined
async function initializeVideoDecoder(config: VideoDecoderConfig) {
  function sendVideoFrameMessage(frame: VideoFrame): void {
    console.debug('[videoDecoder] decoded frame', {
      timestamp: frame.timestamp,
      duration: frame.duration ?? null,
      codedWidth: frame.codedWidth,
      codedHeight: frame.codedHeight,
      displayWidth: frame.displayWidth,
      displayHeight: frame.displayHeight
    })
    self.postMessage({
      type: 'frame',
      frame,
      width: frame.displayWidth,
      height: frame.displayHeight
    })
    frame.close()
  }

  const init: VideoDecoderInit = {
    output: sendVideoFrameMessage,
    error: (e: any) => {
      console.log(e.message)
      videoDecoder = undefined
      cachedVideoConfig = null
    }
  }
  const decoder = new VideoDecoder(init)
  VideoDecoder.isConfigSupported(config)
    .then((support) => {
      console.log('[videoDecoder] config supported', support)
    })
    .catch((err) => {
      console.warn('[videoDecoder] config support check failed', err)
    })
  decoder.configure(config)
  console.info('[videoDecoder] (re)initializing decoder with config:', config)
  return decoder
}

type VideoJitterBufferConfig = {
  mode?: VideoJitterBufferMode
  minDelayMs?: number
  maxBufferSize?: number
  bufferedAheadFrames?: number
}

const POP_INTERVAL_MS = 33
type NormalizedJitterConfig = {
  mode: VideoJitterBufferMode
  minDelayMs: number
  maxBufferSize: number
  bufferedAheadFrames: number
}

const DEFAULT_JITTER_CONFIG: NormalizedJitterConfig = {
  maxBufferSize: 9000,
  mode: 'fast',
  minDelayMs: 250,
  bufferedAheadFrames: 5
}

let jitterBuffer = createJitterBuffer()
let currentJitterConfig: NormalizedJitterConfig = { ...DEFAULT_JITTER_CONFIG }

function normalizeJitterConfig(config?: VideoJitterBufferConfig): NormalizedJitterConfig {
  const mode = config?.mode ?? DEFAULT_JITTER_CONFIG.mode
  const minDelayMs =
    typeof config?.minDelayMs === 'number' && Number.isFinite(config.minDelayMs) && config.minDelayMs >= 0
      ? config.minDelayMs
      : DEFAULT_JITTER_CONFIG.minDelayMs
  const maxBufferSize =
    typeof config?.maxBufferSize === 'number' && Number.isFinite(config.maxBufferSize) && config.maxBufferSize > 0
      ? Math.floor(config.maxBufferSize)
      : DEFAULT_JITTER_CONFIG.maxBufferSize
  const bufferedAheadFrames =
    typeof config?.bufferedAheadFrames === 'number' &&
    Number.isFinite(config.bufferedAheadFrames) &&
    config.bufferedAheadFrames > 0
      ? Math.floor(config.bufferedAheadFrames)
      : DEFAULT_JITTER_CONFIG.bufferedAheadFrames
  return { mode, minDelayMs, maxBufferSize, bufferedAheadFrames }
}

function createJitterBuffer(config?: VideoJitterBufferConfig): VideoJitterBuffer {
  const merged = normalizeJitterConfig(config)
  const buffer = new VideoJitterBuffer(merged.maxBufferSize, merged.mode, KEYFRAME_INTERVAL_BIGINT)
  buffer.setMinDelay(merged.minDelayMs)
  buffer.setBufferedAheadFrames(merged.bufferedAheadFrames)
  return buffer
}

function updateJitterBuffer(config: VideoJitterBufferConfig): void {
  currentJitterConfig = normalizeJitterConfig({ ...currentJitterConfig, ...config })
  jitterBuffer = createJitterBuffer(currentJitterConfig)
}

type DecodedState = {
  groupId: bigint
  objectId: bigint
}

let lastDecodedState: DecodedState | null = null
let previousGroupClosed = false
type CachedVideoConfig = { codec: string; descriptionBase64?: string; avcFormat?: 'annexb' | 'avc' }

let cachedVideoConfig: CachedVideoConfig | null = null
let pendingReconfigureConfig: CachedVideoConfig | null = null
let catalogCodec: string | null = null
let lastVideoTimestamp: number | null = null
let descriptionLogged = false
let lastTimestampState: { timestamp: number; groupId: bigint; objectId: bigint } | null = null

function checkTimestampMonotonic(timestamp: number, groupId: bigint, objectId: bigint): void {
  if (lastTimestampState && timestamp < lastTimestampState.timestamp) {
    console.warn(
      `[videoDecoder] timestamp regression detected: prev=${lastTimestampState.timestamp} (group=${lastTimestampState.groupId.toString()} object=${lastTimestampState.objectId.toString()}) current=${timestamp} (group=${groupId.toString()} object=${objectId.toString()})`
    )
  }
  lastVideoTimestamp = timestamp
  lastTimestampState = { timestamp, groupId, objectId }
}

// objectIdの連続性をチェック（JitterBufferがcorrectlyモードの場合は冗長だが、念のため保持）
function checkObjectIdContinuity(currentGroupId: bigint, currentObjectId: bigint): void {
  // 初回はgroupId=0, objectId=0であることを確認（キーフレーム）
  if (!lastDecodedState) {
    if (currentGroupId !== 0n || currentObjectId !== 0n) {
      console.warn(
        `[Video] First frame must be groupId=0, objectId=0 (keyframe). Got: groupId=${currentGroupId}, objectId=${currentObjectId}`
      )
    }
    return
  }

  // groupIdが変わった場合: 前回のobjectIdが最後のdeltaframeかチェック
  if (currentGroupId !== lastDecodedState.groupId) {
    if (!previousGroupClosed) {
      const expectedLastObjectId = KEYFRAME_INTERVAL_BIGINT - 1n
      if (lastDecodedState.objectId !== expectedLastObjectId) {
        console.debug(
          `[Video] Group ended with unexpected objectId. Expected: ${expectedLastObjectId}, Got: ${lastDecodedState.objectId}, Group: ${lastDecodedState.groupId} -> ${currentGroupId}`
        )
      }
    }
    // 新しいgroupの最初のobjectIdは0であるべき
    if (currentObjectId !== 0n) {
      console.warn(
        `[Video] New group should start with objectId 0. Got: ${currentObjectId}, GroupId: ${currentGroupId}`
      )
    }
    return
  }

  // 同一group内での連続性チェック
  if (currentObjectId !== lastDecodedState.objectId + 1n) {
    console.warn(
      `[Video] Non-sequential objectId detected. Expected: ${lastDecodedState.objectId + 1n}, Got: ${currentObjectId}, Gap: ${
        currentObjectId - lastDecodedState.objectId - 1n
      }`
    )
  }
}

function recordDecodedFrame(groupId: bigint, objectId: bigint): void {
  previousGroupClosed = false
  lastDecodedState = { groupId, objectId }
}

function markGroupClosed(): void {
  previousGroupClosed = true
}

setInterval(() => {
  const entry = jitterBuffer.popWithMetadata()
  if (entry) {
    if (entry.isEndOfGroup) {
      markGroupClosed()
      return
    }
    decode(entry.groupId, entry.object)
  }
}, POP_INTERVAL_MS)

type DecoderControlMessage = { type: 'config'; config: VideoJitterBufferConfig } | { type: 'catalog'; codec?: string }
type WorkerMessage = SubgroupWorkerMessage | DecoderControlMessage

function isConfigMessage(message: WorkerMessage): message is { type: 'config'; config: VideoJitterBufferConfig } {
  return (message as { type?: string }).type === 'config'
}

function isCatalogMessage(message: WorkerMessage): message is { type: 'catalog'; codec?: string } {
  return (message as { type?: string }).type === 'catalog'
}

self.onmessage = async (event: MessageEvent<WorkerMessage>) => {
  if (isConfigMessage(event.data)) {
    updateJitterBuffer(event.data.config)
    return
  }
  if (isCatalogMessage(event.data)) {
    applyCatalogCodec(event.data.codec)
    return
  }

  const locHeader = event.data.subgroupStreamObject.locHeader
  const locExtensions = locHeader?.extensions ?? []
  const hasCaptureTimestamp = locExtensions.some((ext) => ext.type === 'captureTimestamp')
  const hasVideoConfig = locExtensions.some((ext) => ext.type === 'videoConfig')
  const hasVideoFrameMarking = locExtensions.some((ext) => ext.type === 'videoFrameMarking')
  console.debug('[videoDecoder] recv object', {
    groupId: event.data.groupId,
    objectId: event.data.subgroupStreamObject.objectId,
    payloadLength: event.data.subgroupStreamObject.objectPayloadLength,
    status: event.data.subgroupStreamObject.objectStatus,
    locExtensionCount: locExtensions.length,
    hasCaptureTimestamp,
    hasVideoConfig,
    hasVideoFrameMarking
  })
  const subgroupStreamObject: SubgroupObjectWithLoc = {
    objectId: event.data.subgroupStreamObject.objectId,
    objectPayloadLength: event.data.subgroupStreamObject.objectPayloadLength,
    objectPayload: new Uint8Array(event.data.subgroupStreamObject.objectPayload),
    objectStatus: event.data.subgroupStreamObject.objectStatus,
    locHeader: event.data.subgroupStreamObject.locHeader
  }
  bitrateLogger.addBytes(subgroupStreamObject.objectPayloadLength)

  jitterBuffer.push(event.data.groupId, subgroupStreamObject.objectId, subgroupStreamObject, (latencyMs) =>
    postReceiveLatency(latencyMs)
  )
}

async function decode(groupId: bigint, subgroupStreamObject: JitterBufferSubgroupObject) {
  // objectIdの連続性をチェック
  checkObjectIdContinuity(groupId, subgroupStreamObject.objectId)
  recordDecodedFrame(groupId, subgroupStreamObject.objectId)

  const decoded = subgroupStreamObject.cachedChunk
  reportLatency(decoded.metadata.sentAt)

  const resolvedConfig = resolveVideoConfig(decoded.metadata)
  if (!resolvedConfig) {
    return
  }
  if (decoded.metadata.type === 'key') {
    const payloadInfo = analyzeKeyframePayload(decoded.data)
    const prevTimestamp = lastVideoTimestamp
    const delta = prevTimestamp !== null ? decoded.metadata.timestamp - prevTimestamp : null
    console.debug('[videoDecoder] keyframe timestamp', {
      groupId: groupId.toString(),
      objectId: subgroupStreamObject.objectId.toString(),
      timestamp: decoded.metadata.timestamp,
      duration: decoded.metadata.duration ?? null,
      delta
    })
    console.debug('[videoDecoder] keyframe payload', {
      length: decoded.data.byteLength,
      head: payloadInfo.head,
      hasStartCode: payloadInfo.hasStartCode,
      startCodeAtZero: payloadInfo.startCodeAtZero,
      nalTypes: payloadInfo.nalSummary
    })
  }
  checkTimestampMonotonic(decoded.metadata.timestamp, groupId, subgroupStreamObject.objectId)
  const desiredConfig = buildVideoDecoderConfig(resolvedConfig)

  const needsReconfigure = shouldReconfigure(resolvedConfig)
  if (needsReconfigure && decoded.metadata.type !== 'key') {
    pendingReconfigureConfig = resolvedConfig
    return
  }
  if (pendingReconfigureConfig && decoded.metadata.type === 'key') {
    if (shouldReconfigure(pendingReconfigureConfig)) {
      await reinitializeDecoder(buildVideoDecoderConfig(pendingReconfigureConfig), pendingReconfigureConfig)
    }
    pendingReconfigureConfig = null
    // fallthrough to decode this keyframe
  }

  const encodedVideoChunk = new EncodedVideoChunk({
    type: decoded.metadata.type as EncodedVideoChunkType,
    timestamp: decoded.metadata.timestamp,
    duration: decoded.metadata.duration ?? undefined,
    data: decoded.data
  })

  if (!videoDecoder || videoDecoder.state === 'closed') {
    videoDecoder = await initializeVideoDecoder(desiredConfig)
    cachedVideoConfig = resolvedConfig
    postDecoderConfig(resolvedConfig, desiredConfig)
    if (decoded.metadata.type !== 'key') {
      return
    }
  }

  if (needsReconfigure) {
    await reinitializeDecoder(desiredConfig, resolvedConfig)
    if (decoded.metadata.type !== 'key') {
      return
    }
  }

  await videoDecoder.decode(encodedVideoChunk)
}

async function reinitializeDecoder(desiredConfig: VideoDecoderConfig, resolvedConfig: CachedVideoConfig) {
  try {
    videoDecoder?.close()
  } catch (e) {
    console.warn('[videoDecoder] failed to close before reconfigure', e)
  }
  videoDecoder = await initializeVideoDecoder(desiredConfig)
  console.log('[videoDecoder] reinitialize with config:', desiredConfig)
  cachedVideoConfig = resolvedConfig
  postDecoderConfig(resolvedConfig, desiredConfig)
}

function reportLatency(sentAt: number | undefined) {
  if (typeof sentAt !== 'number') {
    return
  }
  const latency = Date.now() - sentAt
  if (latency < 0) {
    return
  }
  postRenderingLatency(latency)
}

function postReceiveLatency(latencyMs: number) {
  if (latencyMs < 0) {
    return
  }
  self.postMessage({ type: 'receiveLatency', media: 'video', ms: latencyMs })
}

function postRenderingLatency(latencyMs: number) {
  if (latencyMs < 0) {
    return
  }
  self.postMessage({ type: 'renderingLatency', media: 'video', ms: latencyMs })
}

function postDecoderConfig(resolvedConfig: CachedVideoConfig, desired: VideoDecoderConfig) {
  const descriptionLength = desired.description instanceof Uint8Array ? desired.description.byteLength : undefined
  self.postMessage({
    type: 'decoderConfig',
    codec: resolvedConfig.codec,
    descriptionLength
  })
}

function applyCatalogCodec(codec?: string): void {
  if (!codec) {
    return
  }
  if (catalogCodec === codec) {
    return
  }
  catalogCodec = codec
  if (!cachedVideoConfig) {
    return
  }
  if (cachedVideoConfig.codec === codec) {
    return
  }
  pendingReconfigureConfig = {
    codec,
    descriptionBase64: cachedVideoConfig.descriptionBase64,
    avcFormat: cachedVideoConfig.avcFormat
  }
}

function resolveVideoConfig(metadata: ChunkMetadata): CachedVideoConfig | null {
  const hasNewConfig = metadata.codec || metadata.descriptionBase64 || metadata.avcFormat
  if (!hasNewConfig && !cachedVideoConfig) {
    return { codec: VIDEO_DECODER_CONFIG.codec }
  }
  const codec = metadata.codec ?? cachedVideoConfig?.codec ?? catalogCodec ?? undefined
  const descriptionBase64 = metadata.descriptionBase64 ?? cachedVideoConfig?.descriptionBase64
  const avcFormat = metadata.avcFormat ?? cachedVideoConfig?.avcFormat
  if (!codec) {
    return null
  }
  return { codec, descriptionBase64, avcFormat }
}

function buildVideoDecoderConfig(resolved: CachedVideoConfig): VideoDecoderConfig {
  if (resolved.codec.startsWith('avc')) {
    const description = resolved.descriptionBase64 ? base64ToUint8Array(resolved.descriptionBase64) : undefined
    logDescriptionBytes(description)
    return {
      ...VIDEO_DECODER_CONFIG,
      codec: resolved.codec,
      description,
      avc: {
        format: resolved.avcFormat ?? 'avc'
      }
    } as any
  }

  return {
    codec: resolved.codec,
    description: resolved.descriptionBase64 ? base64ToUint8Array(resolved.descriptionBase64) : undefined
  } as VideoDecoderConfig
}

function shouldReconfigure(resolved: CachedVideoConfig): boolean {
  if (!cachedVideoConfig) return true
  return (
    cachedVideoConfig.codec !== resolved.codec ||
    cachedVideoConfig.descriptionBase64 !== resolved.descriptionBase64 ||
    cachedVideoConfig.avcFormat !== resolved.avcFormat
  )
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

function logDescriptionBytes(description?: Uint8Array): void {
  if (!description || descriptionLogged) {
    return
  }
  const head = formatHexPrefix(description, 8)
  console.log('[videoDecoder] description bytes', { length: description.byteLength, head })
  descriptionLogged = true
}

function formatHexPrefix(bytes: Uint8Array, maxLen: number): string {
  const parts: string[] = []
  const size = Math.min(bytes.byteLength, maxLen)
  for (let i = 0; i < size; i += 1) {
    parts.push(bytes[i].toString(16).padStart(2, '0').toUpperCase())
  }
  return parts.join(' ')
}

function analyzeKeyframePayload(data: Uint8Array): {
  head: string
  hasStartCode: boolean
  startCodeAtZero: boolean
  nalSummary: string
} {
  const head = formatHexPrefix(data, 12)
  const nalInfo = collectAnnexbNalTypes(data)
  return {
    head,
    hasStartCode: nalInfo.hasStartCode,
    startCodeAtZero: nalInfo.startCodeAtZero,
    nalSummary: nalInfo.types.length ? nalInfo.types.join(',') : 'none'
  }
}

function collectAnnexbNalTypes(data: Uint8Array): {
  hasStartCode: boolean
  startCodeAtZero: boolean
  types: number[]
} {
  const start = findStartCode(data, 0)
  if (!start) {
    return { hasStartCode: false, startCodeAtZero: false, types: [] }
  }
  const types: number[] = []
  let pos = start.index + start.length
  let next = pos
  while (true) {
    const nextStart = findStartCode(data, next)
    const end = nextStart ? nextStart.index : data.length
    if (end > pos) {
      types.push(data[pos] & 0x1f)
    }
    if (!nextStart) {
      break
    }
    pos = nextStart.index + nextStart.length
    next = pos
  }
  return { hasStartCode: true, startCodeAtZero: start.index === 0, types }
}

function findStartCode(data: Uint8Array, offset: number): { index: number; length: number } | null {
  for (let i = offset; i + 3 <= data.length; i += 1) {
    if (data[i] === 0 && data[i + 1] === 0) {
      if (data[i + 2] === 1) {
        return { index: i, length: 3 }
      }
      if (i + 3 < data.length && data[i + 2] === 0 && data[i + 3] === 1) {
        return { index: i, length: 4 }
      }
    }
  }
  return null
}
