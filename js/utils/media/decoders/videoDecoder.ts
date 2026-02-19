import { VideoJitterBuffer, type VideoJitterBufferMode } from '../videoJitterBuffer'
import type { JitterBufferSubgroupObject, SubgroupObjectWithLoc, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { KEYFRAME_INTERVAL } from '../constants'
import { createBitrateLogger } from '../bitrate'
import type { ChunkMetadata } from '../chunk'
import { latencyMsFromCaptureMicros } from '../clock'

const bitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', kbps })
})

const KEYFRAME_INTERVAL_BIGINT = BigInt(KEYFRAME_INTERVAL)

const VIDEO_DECODER_CONFIG = {
  hardwareAcceleration: 'prefer-hardware' as any,
  scalabilityMode: 'L1T1'
}

let videoDecoder: VideoDecoder | undefined
let waitingForKeyFrame = true
let framesSinceLastKeyFrame: number | null = null
async function initializeVideoDecoder(config: VideoDecoderConfig) {
  function sendVideoFrameMessage(frame: VideoFrame): void {
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
      console.warn('[videoDecoder] decoder error', e)
      videoDecoder = undefined
      cachedVideoConfig = null
    }
  }
  const decoder = new VideoDecoder(init)
  decoder.configure(config)
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

type CachedVideoConfig = { codec: string; descriptionBase64?: string; avcFormat?: 'annexb' | 'avc' }

let cachedVideoConfig: CachedVideoConfig | null = null
let catalogCodec: string | null = null
let missingCatalogCodecWarned = false

setInterval(() => {
  const entry = jitterBuffer.popWithMetadata()
  if (entry) {
    postJitterBufferActivity('pop')
    if (entry.isEndOfGroup) {
      return
    }
    decode(entry.groupId, entry.object, entry.captureTimestampMicros)
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
  const subgroupStreamObject: SubgroupObjectWithLoc = {
    objectId: event.data.subgroupStreamObject.objectId,
    objectPayloadLength: event.data.subgroupStreamObject.objectPayloadLength,
    objectPayload: new Uint8Array(event.data.subgroupStreamObject.objectPayload),
    objectStatus: event.data.subgroupStreamObject.objectStatus,
    locHeader: event.data.subgroupStreamObject.locHeader
  }
  bitrateLogger.addBytes(subgroupStreamObject.objectPayloadLength)

  const inserted = jitterBuffer.push(
    event.data.groupId,
    subgroupStreamObject.objectId,
    subgroupStreamObject,
    (latencyMs) => postReceiveLatency(latencyMs)
  )
  if (inserted) {
    postJitterBufferActivity('push')
  }
}

async function decode(
  groupId: bigint,
  subgroupStreamObject: JitterBufferSubgroupObject,
  captureTimestampMicros?: number
) {
  const decoded = subgroupStreamObject.cachedChunk
  trackKeyframeInterval(decoded.metadata.type)
  reportLatency(captureTimestampMicros)

  const resolvedConfig = resolveVideoConfig(decoded.metadata)
  if (!resolvedConfig) {
    return
  }
  const desiredConfig = buildVideoDecoderConfig(resolvedConfig)

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
    waitingForKeyFrame = true
    if (decoded.metadata.type !== 'key') {
      return
    }
  }

  if (waitingForKeyFrame && decoded.metadata.type !== 'key') {
    return
  }

  try {
    await videoDecoder.decode(encodedVideoChunk)
    if (decoded.metadata.type === 'key') {
      waitingForKeyFrame = false
    }
  } catch (error) {
    if (isKeyFrameRequiredError(error)) {
      waitingForKeyFrame = true
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

function trackKeyframeInterval(chunkType: string): void {
  if (framesSinceLastKeyFrame !== null) {
    framesSinceLastKeyFrame += 1
  }
  if (chunkType !== 'key') {
    return
  }
  if (typeof framesSinceLastKeyFrame === 'number' && framesSinceLastKeyFrame > 0) {
    self.postMessage({ type: 'keyframeInterval', media: 'video', frames: framesSinceLastKeyFrame })
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
  self.postMessage({ type: 'receiveLatency', media: 'video', ms: latencyMs })
}

function postRenderingLatency(latencyMs: number) {
  if (latencyMs < 0) {
    return
  }
  self.postMessage({ type: 'renderingLatency', media: 'video', ms: latencyMs })
}

function postJitterBufferActivity(event: 'push' | 'pop') {
  self.postMessage({
    type: 'jitterBufferActivity',
    media: 'video',
    event,
    bufferedFrames: jitterBuffer.getBufferedFrameCount(),
    capacityFrames: jitterBuffer.getMaxBufferSize()
  })
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
    avcFormat: metadata.avcFormat
  }
}

function buildVideoDecoderConfig(resolved: CachedVideoConfig): VideoDecoderConfig {
  if (resolved.codec.startsWith('avc')) {
    const description = resolved.descriptionBase64 ? base64ToUint8Array(resolved.descriptionBase64) : undefined
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

function base64ToUint8Array(base64: string): Uint8Array {
  const binaryString = atob(base64)
  const len = binaryString.length
  const bytes = new Uint8Array(len)
  for (let i = 0; i < len; i++) {
    bytes[i] = binaryString.charCodeAt(i)
  }
  return bytes
}
