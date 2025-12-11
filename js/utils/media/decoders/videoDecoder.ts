import { VideoJitterBuffer, type VideoJitterBufferMode } from '../videoJitterBuffer'
import type { JitterBufferSubgroupObject, SubgroupObject, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { KEYFRAME_INTERVAL } from '../constants'
import { createBitrateLogger } from '../bitrate'
import type { ChunkMetadata } from '../chunk'

const bitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', kbps })
})

const KEYFRAME_INTERVAL_BIGINT = BigInt(KEYFRAME_INTERVAL)

const VIDEO_DECODER_CONFIG = {
  //codec: 'av01.0.08M.08',
  codec: 'avc1.640028',
  avc: {
    format: 'annexb'
  } as any,
  hardwareAcceleration: 'prefer-hardware' as any,
  width: 1920,
  height: 1080,
  scalabilityMode: 'L1T1'
}

let videoDecoder: VideoDecoder | undefined
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
      console.log(e.message)
      videoDecoder = undefined
    }
  }
  const decoder = new VideoDecoder(init)
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
let cachedVideoConfig: { codec: string; descriptionBase64?: string } | null = null
let pendingReconfigureConfig: { codec: string; descriptionBase64?: string } | null = null

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

type WorkerMessage = SubgroupWorkerMessage | { type: 'config'; config: VideoJitterBufferConfig }

function isConfigMessage(message: WorkerMessage): message is { type: 'config'; config: VideoJitterBufferConfig } {
  return (message as { type?: string }).type === 'config'
}

self.onmessage = async (event: MessageEvent<WorkerMessage>) => {
  if (isConfigMessage(event.data)) {
    updateJitterBuffer(event.data.config)
    return
  }

  const subgroupStreamObject: SubgroupObject = {
    objectId: event.data.subgroupStreamObject.objectId,
    objectPayloadLength: event.data.subgroupStreamObject.objectPayloadLength,
    objectPayload: new Uint8Array(event.data.subgroupStreamObject.objectPayload),
    objectStatus: event.data.subgroupStreamObject.objectStatus
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

async function reinitializeDecoder(
  desiredConfig: VideoDecoderConfig,
  resolvedConfig: { codec: string; descriptionBase64?: string }
) {
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

function postDecoderConfig(resolvedConfig: { codec: string; descriptionBase64?: string }, desired: VideoDecoderConfig) {
  const descriptionLength = desired.description instanceof Uint8Array ? desired.description.byteLength : undefined
  self.postMessage({
    type: 'decoderConfig',
    codec: resolvedConfig.codec,
    descriptionLength
  })
}

function resolveVideoConfig(metadata: ChunkMetadata): { codec: string; descriptionBase64?: string } | null {
  const hasNewConfig = metadata.codec || metadata.descriptionBase64
  if (!hasNewConfig && !cachedVideoConfig) {
    return null
  }
  const codec = metadata.codec ?? cachedVideoConfig?.codec
  const descriptionBase64 = metadata.descriptionBase64 ?? cachedVideoConfig?.descriptionBase64
  if (!codec) {
    return null
  }
  return { codec, descriptionBase64 }
}

function buildVideoDecoderConfig(resolved: { codec: string; descriptionBase64?: string }): VideoDecoderConfig {
  if (resolved.codec.startsWith('avc')) {
    const description = resolved.descriptionBase64 ? base64ToUint8Array(resolved.descriptionBase64) : undefined
    return {
      ...VIDEO_DECODER_CONFIG,
      codec: resolved.codec,
      description
    }
  }

  return {
    codec: resolved.codec,
    description: resolved.descriptionBase64 ? base64ToUint8Array(resolved.descriptionBase64) : undefined
  } as VideoDecoderConfig
}

function shouldReconfigure(resolved: { codec: string; descriptionBase64?: string }): boolean {
  if (!cachedVideoConfig) return true
  return (
    cachedVideoConfig.codec !== resolved.codec || cachedVideoConfig.descriptionBase64 !== resolved.descriptionBase64
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
