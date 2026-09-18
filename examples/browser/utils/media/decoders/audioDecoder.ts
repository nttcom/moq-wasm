import { AudioJitterBuffer } from '../audioJitterBuffer'
import { GroupOrderGate } from '../groupOrderGate'
import { base64ToUint8Array } from '../base64'
import type { SubgroupObjectWithLoc, JitterBufferSubgroupObject, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { createBitrateLogger } from '../bitrate'
import { type ChunkMetadata } from '../chunk'
import { latencyMsFromCaptureMicros } from '../clock'
import { readLocHeader } from '../loc'
import { buildAudioChunkFromLoc } from '../locChunk'
import { trackSubgroupObjectId } from '../subgroupObjectId'

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
/// The decoder stamps its outputs from the sample count it has produced, not
/// from the chunk timestamps, so a hole in the source would shift every later
/// output; each output is labelled with the capture timestamp of the chunk it
/// was decoded from instead.
const pendingCaptureTimestamps: (number | undefined)[] = []
let decoderSignature: string | null = null
let cachedAudioConfig: CachedAudioConfig | null = null
let catalogAudioCodec: string | undefined
let catalogAudioSampleRate: number | undefined
let catalogAudioChannels: number | undefined
let catalogAudioDescriptionBase64: string | undefined
let directDecodeQueue: Promise<void> = Promise.resolve()
const directLastObjectIds = new Map<string, bigint>()
const groupOrder = new GroupOrderGate((groupId, object) => enqueueDirectDecode(groupId, object))

function postAudioData(audioData: AudioData, captureTimestampMicros: number | undefined): void {
  self.postMessage({ type: 'audioData', audioData, captureTimestampMicros }, [audioData])
}

async function createAudioDecoder(config: AudioDecoderConfig, signature: string) {
  const init: AudioDecoderInit = {
    output: (audioData) => postAudioData(audioData, pendingCaptureTimestamps.shift()),
    error: (e: any) => {
      console.warn('[audioDecoder] decoder error', e)
    }
  }
  const decoder = new AudioDecoder(init)
  decoder.configure(config)
  decoderSignature = signature
  pendingCaptureTimestamps.length = 0
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

type AudioCatalogMessage = {
  type: 'catalog'
  codec?: string
  sampleRate?: number
  channels?: number
  descriptionBase64?: string
}

type AudioWorkerMessage =
  | SubgroupWorkerMessage
  | { type: 'config'; config: { mode?: string; telemetryEnabled?: boolean; bypassJitterBuffer?: boolean } }
  | AudioCatalogMessage

self.onmessage = async (event: MessageEvent<AudioWorkerMessage>) => {
  if ((event.data as { type?: string }).type === 'config') {
    const config = (
      event.data as {
        type: 'config'
        config: { mode?: string; telemetryEnabled?: boolean; bypassJitterBuffer?: boolean }
      }
    ).config
    telemetryEnabled = config.telemetryEnabled ?? telemetryEnabled
    bypassJitterBuffer = config.bypassJitterBuffer ?? bypassJitterBuffer
    if (config.mode === 'ordered' || config.mode === 'latest') {
      jitterBuffer.setMode(config.mode)
    }
    return
  }

  if ((event.data as { type?: string }).type === 'catalog') {
    const catalog = event.data as AudioCatalogMessage
    if (catalog.codec) {
      catalogAudioCodec = catalog.codec
    }
    if (typeof catalog.sampleRate === 'number') {
      catalogAudioSampleRate = catalog.sampleRate
    }
    if (typeof catalog.channels === 'number') {
      catalogAudioChannels = catalog.channels
    }
    if (catalog.descriptionBase64) {
      catalogAudioDescriptionBase64 = catalog.descriptionBase64
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
    groupOrder.push(message.groupId, subgroupStreamObject)
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
  const objectId = trackSubgroupObjectId(directLastObjectIds, groupId, object)
  if (!object.objectPayloadLength) {
    return null
  }
  const loc = readLocHeader(object.locHeader)
  const captureTimestampMicros = loc.captureTimestampMicros
  const parsed = buildAudioChunkFromLoc(loc, object.objectPayload)

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

async function decode(subgroupStreamObject: JitterBufferSubgroupObject, captureTimestampMicros?: number) {
  const decoded = subgroupStreamObject.cachedChunk
  reportAudioLatency(captureTimestampMicros)

  const resolvedConfig = resolveAudioConfig(decoded.metadata)
  if (!resolvedConfig) {
    // メタデータからデコーダ設定を導けるまで待つ
    return
  }
  const desiredSignature = buildSignature(resolvedConfig)
  if (isPcmAlawCodec(resolvedConfig.codec)) {
    try {
      decodePcmAlawChunk(decoded.metadata, decoded.data, resolvedConfig, captureTimestampMicros)
      cachedAudioConfig = resolvedConfig
      if (audioDecoder && audioDecoder.state !== 'closed') {
        audioDecoder.close()
      }
      audioDecoder = undefined
      decoderSignature = desiredSignature
    } catch (error) {
      console.error('[audioDecoder] pcm alaw decode failed', error)
    }
    return
  }
  const desiredConfig = buildDecoderConfig(resolvedConfig)

  if (!audioDecoder || audioDecoder.state === 'closed' || decoderSignature !== desiredSignature) {
    try {
      if (audioDecoder && audioDecoder.state !== 'closed') {
        audioDecoder.close()
      }
      audioDecoder = await createAudioDecoder(desiredConfig, desiredSignature)
      cachedAudioConfig = resolvedConfig
    } catch (e) {
      console.error('[audioDecoder] configure failed', e)
      return
    }
  }

  const encodedAudioChunk = new EncodedAudioChunk({
    type: decoded.metadata.type as EncodedAudioChunkType,
    timestamp: decoded.metadata.timestamp,
    duration: decoded.metadata.duration ?? undefined,
    data: decoded.data
  })

  audioDecoder.decode(encodedAudioChunk)
  pendingCaptureTimestamps.push(captureTimestampMicros)
}

function decodePcmAlawChunk(
  metadata: ChunkMetadata,
  payload: Uint8Array,
  resolved: CachedAudioConfig,
  captureTimestampMicros: number | undefined
): void {
  const channels = Math.max(1, resolved.channels)
  const sampleRate = Math.max(1, resolved.sampleRate)
  const totalSamples = payload.byteLength
  const numberOfFrames = Math.floor(totalSamples / channels)
  if (numberOfFrames <= 0) {
    return
  }

  const pcm = new Int16Array(numberOfFrames * channels)
  for (let i = 0; i < pcm.length; i += 1) {
    pcm[i] = decodeAlawSample(payload[i] ?? 0)
  }

  const audioData = new AudioData({
    format: 's16',
    sampleRate,
    numberOfFrames,
    numberOfChannels: channels,
    timestamp: metadata.timestamp,
    data: new Uint8Array(pcm.buffer)
  })
  postAudioData(audioData, captureTimestampMicros)
}

function isPcmAlawCodec(codec: string): boolean {
  const normalized = codec.trim().toLowerCase()
  return normalized === 'pcma' || normalized === 'pcm_alaw' || normalized === 'g711-alaw'
}

function decodeAlawSample(encoded: number): number {
  let sample = encoded ^ 0x55
  let value = (sample & 0x0f) << 4
  const segment = (sample & 0x70) >> 4
  switch (segment) {
    case 0:
      value += 8
      break
    case 1:
      value += 0x108
      break
    default:
      value += 0x108
      value <<= segment - 1
      break
  }
  return (sample & 0x80) !== 0 ? value : -value
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

function resolveAudioConfig(metadata: ChunkMetadata): CachedAudioConfig | null {
  const hasNewConfig =
    metadata.codec ||
    metadata.descriptionBase64 ||
    metadata.sampleRate ||
    metadata.channels ||
    catalogAudioCodec ||
    catalogAudioSampleRate ||
    catalogAudioChannels ||
    catalogAudioDescriptionBase64
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
    catalogAudioCodec ??
    cachedAudioConfig?.codec ??
    DEFAULT_AUDIO_DECODER_CONFIG.codec
  const sampleRate =
    metadata.sampleRate ??
    catalogAudioSampleRate ??
    cachedAudioConfig?.sampleRate ??
    DEFAULT_AUDIO_DECODER_CONFIG.sampleRate
  const channels =
    metadata.channels ??
    catalogAudioChannels ??
    cachedAudioConfig?.channels ??
    DEFAULT_AUDIO_DECODER_CONFIG.numberOfChannels
  const descriptionBase64 =
    metadata.descriptionBase64 ?? catalogAudioDescriptionBase64 ?? cachedAudioConfig?.descriptionBase64

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

function buildSignature(resolved: CachedAudioConfig): string {
  const desc = resolved.descriptionBase64 ?? ''
  return `${resolved.codec}:${resolved.sampleRate}:${resolved.channels}:${desc}`
}
