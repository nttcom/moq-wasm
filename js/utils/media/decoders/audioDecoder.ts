import { AudioJitterBuffer } from '../audioJitterBuffer'
import type { SubgroupObjectWithLoc, JitterBufferSubgroupObject, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { createBitrateLogger } from '../bitrate'
import { tryDeserializeChunk, type ChunkMetadata } from '../chunk'
import { latencyMsFromCaptureMicros } from '../clock'
import { readLocHeader } from '../loc'

let telemetryEnabled = true
let bypassJitterBuffer = false

function postTelemetry(message: unknown): void {
  if (!telemetryEnabled) {
    return
  }
  self.postMessage(message)
}

type CachedAudioConfig = {
  codec: string
  sampleRate: number
  channels: number
  descriptionBase64?: string
}

const audioBitrateLogger = createBitrateLogger((kbps) => {
  postTelemetry({ type: 'bitrate', media: 'audio', kbps })
})

const DEFAULT_AUDIO_DECODER_CONFIG = {
  codec: 'opus',
  sampleRate: 48000, // Opusの推奨サンプルレート
  numberOfChannels: 1 // モノラル
}

let audioDecoder: AudioDecoder | undefined
let remoteTimestampBase: number | null = null
let decoderSignature: string | null = null
let cachedAudioConfig: CachedAudioConfig | null = null
let directDecodeQueue: Promise<void> = Promise.resolve()
const directLastObjectIds = new Map<string, bigint>()

async function createAudioDecoder(config: AudioDecoderConfig, signature: string) {
  function sendAudioDataMessage(audioData: AudioData): void {
    self.postMessage({ type: 'audioData', audioData }, [audioData])
  }

  const init: AudioDecoderInit = {
    output: sendAudioDataMessage,
    error: (e: any) => {
      console.warn('[audioDecoder] decoder error', e)
    }
  }
  const decoder = new AudioDecoder(init)
  decoder.configure(config)
  decoderSignature = signature
  return decoder
}

const POP_INTERVAL_MS = 5
const jitterBuffer = new AudioJitterBuffer(1800, 'ordered')

function postBufferedObject(groupId: bigint, objectId: bigint) {
  postTelemetry({
    type: 'bufferedObject',
    media: 'audio',
    groupId,
    objectId
  })
}

setInterval(() => {
  const jitterBufferEntry = jitterBuffer.pop()
  if (!jitterBufferEntry) {
    return
  }
  postJitterBufferActivity('pop')
  const subgroupStreamObject = jitterBufferEntry?.object
  if (subgroupStreamObject) {
    decode(subgroupStreamObject, jitterBufferEntry.captureTimestampMicros)
  }
}, POP_INTERVAL_MS)

type AudioWorkerMessage =
  | SubgroupWorkerMessage
  | { type: 'config'; config: { mode?: string; telemetryEnabled?: boolean; bypassJitterBuffer?: boolean } }

self.onmessage = async (event: MessageEvent<AudioWorkerMessage>) => {
  if ((event.data as { type?: string }).type === 'config') {
    const config = (event.data as {
      type: 'config'
      config: { mode?: string; telemetryEnabled?: boolean; bypassJitterBuffer?: boolean }
    }).config
    telemetryEnabled = config.telemetryEnabled ?? telemetryEnabled
    bypassJitterBuffer = config.bypassJitterBuffer ?? bypassJitterBuffer
    if (config.mode === 'ordered' || config.mode === 'latest') {
      jitterBuffer.setMode(config.mode)
    }
    return
  }

  const message = event.data as SubgroupWorkerMessage
  const subgroupStreamObject: SubgroupObjectWithLoc = {
    subgroupId: message.subgroupStreamObject.subgroupId,
    objectIdDelta: message.subgroupStreamObject.objectIdDelta,
    objectPayloadLength: message.subgroupStreamObject.objectPayloadLength,
    objectPayload: message.subgroupStreamObject.objectPayload,
    objectStatus: message.subgroupStreamObject.objectStatus,
    locHeader: message.subgroupStreamObject.locHeader
  }
  audioBitrateLogger.addBytes(subgroupStreamObject.objectPayloadLength)

  if (bypassJitterBuffer) {
    enqueueDirectDecode(message.groupId, subgroupStreamObject)
    return
  }

  const objectId = jitterBuffer.push(message.groupId, subgroupStreamObject, (latencyMs) =>
    postReceiveLatency(latencyMs)
  )
  if (objectId !== null) {
    postJitterBufferActivity('push')
    postBufferedObject(message.groupId, objectId)
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
      await decode(entry.object, entry.captureTimestampMicros)
    })
    .catch((error) => {
      console.error('[audioDecoder] direct decode failed', error)
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

  const parsed = tryDeserializeChunk(object.objectPayload) ?? buildChunkFromLoc(object)
  if (!parsed) {
    return null
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
  return status === OBJECT_STATUS_END_OF_GROUP || status === OBJECT_STATUS_END_OF_TRACK
}

async function decode(subgroupStreamObject: JitterBufferSubgroupObject, captureTimestampMicros?: number) {
  const decoded = subgroupStreamObject.cachedChunk
  reportAudioLatency(captureTimestampMicros)

  const resolvedConfig = resolveAudioConfig(decoded.metadata)
  if (!resolvedConfig) {
    // メタデータからデコーダ設定を導けるまで待つ
    return
  }
  const desiredConfig = buildDecoderConfig(resolvedConfig)
  const desiredSignature = buildSignature(resolvedConfig)

  if (!audioDecoder || audioDecoder.state === 'closed' || decoderSignature !== desiredSignature) {
    try {
      if (audioDecoder && audioDecoder.state !== 'closed') {
        audioDecoder.close()
      }
      audioDecoder = await createAudioDecoder(desiredConfig, desiredSignature)
      remoteTimestampBase = null
      cachedAudioConfig = resolvedConfig
    } catch (e) {
      console.error('[audioDecoder] configure failed', e)
      return
    }
  }

  const rebasedTimestamp = rebaseTimestamp(decoded.metadata.timestamp)

  const encodedAudioChunk = new EncodedAudioChunk({
    type: decoded.metadata.type as EncodedAudioChunkType,
    timestamp: rebasedTimestamp,
    duration: decoded.metadata.duration ?? undefined,
    data: decoded.data
  })

  await audioDecoder.decode(encodedAudioChunk)
}

function reportAudioLatency(captureTimestampMicros: number | undefined) {
  if (typeof captureTimestampMicros !== 'number') {
    return
  }
  postRenderingLatency(latencyMsFromCaptureMicros(captureTimestampMicros))
}

function postReceiveLatency(latencyMs: number) {
  if (latencyMs < 0) {
    return
  }
  postTelemetry({ type: 'receiveLatency', media: 'audio', ms: latencyMs })
}

function postRenderingLatency(latencyMs: number) {
  if (latencyMs < 0) {
    return
  }
  postTelemetry({ type: 'renderingLatency', media: 'audio', ms: latencyMs })
}

function postJitterBufferActivity(event: 'push' | 'pop') {
  postTelemetry({
    type: 'jitterBufferActivity',
    media: 'audio',
    event,
    bufferedFrames: jitterBuffer.getBufferedFrameCount(),
    capacityFrames: jitterBuffer.getMaxBufferSize()
  })
}

/**
 * Convert sender-side timestamps to a local timeline so the decoder can play them.
 * NOTE: 検証目的で全てのtimestampを0に設定
 */
function rebaseTimestamp(remoteTimestamp: number): number {
  // 検証目的: 全てのtimestampを0に差し替え
  return 0

  // 元の実装（コメントアウト）
  // if (remoteTimestampBase === null) {
  //   remoteTimestampBase = remoteTimestamp
  // }
  // let rebased = remoteTimestamp - remoteTimestampBase
  // return rebased
}

function resolveAudioConfig(metadata: ChunkMetadata): CachedAudioConfig | null {
  const hasNewConfig = metadata.codec || metadata.descriptionBase64 || metadata.sampleRate || metadata.channels
  if (!hasNewConfig && !cachedAudioConfig) {
    return {
      codec: DEFAULT_AUDIO_DECODER_CONFIG.codec,
      sampleRate: DEFAULT_AUDIO_DECODER_CONFIG.sampleRate,
      channels: DEFAULT_AUDIO_DECODER_CONFIG.numberOfChannels
    }
  }

  const codec =
    metadata.codec ??
    (metadata.descriptionBase64 ? 'mp4a.40.2' : undefined) ??
    cachedAudioConfig?.codec ??
    DEFAULT_AUDIO_DECODER_CONFIG.codec
  const sampleRate = metadata.sampleRate ?? cachedAudioConfig?.sampleRate ?? DEFAULT_AUDIO_DECODER_CONFIG.sampleRate
  const channels = metadata.channels ?? cachedAudioConfig?.channels ?? DEFAULT_AUDIO_DECODER_CONFIG.numberOfChannels
  const descriptionBase64 = metadata.descriptionBase64 ?? cachedAudioConfig?.descriptionBase64

  return { codec, sampleRate, channels, descriptionBase64 }
}

function buildDecoderConfig(resolved: CachedAudioConfig): AudioDecoderConfig {
  if (resolved.codec.startsWith('mp4a')) {
    const description = resolved.descriptionBase64 ? base64ToUint8Array(resolved.descriptionBase64) : undefined
    return {
      codec: resolved.codec,
      sampleRate: resolved.sampleRate,
      numberOfChannels: resolved.channels,
      description
    }
  }

  return {
    codec: DEFAULT_AUDIO_DECODER_CONFIG.codec,
    sampleRate: resolved.sampleRate,
    numberOfChannels: resolved.channels
  }
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

function buildSignature(resolved: CachedAudioConfig): string {
  const desc = resolved.descriptionBase64 ?? ''
  return `${resolved.codec}:${resolved.sampleRate}:${resolved.channels}:${desc}`
}
