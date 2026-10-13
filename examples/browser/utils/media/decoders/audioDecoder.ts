import { GroupOrderGate } from '../groupOrderGate'
import { base64ToUint8Array } from '../base64'
import type { SubgroupObjectWithLoc, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { createBitrateLogger } from '../bitrate'
import type { ChunkMetadata, DeserializedChunk } from '../chunk'
import { readLocHeader } from '../loc'
import { buildAudioChunkFromLoc } from '../locChunk'

type CachedAudioConfig = {
  codec: string
  sampleRate: number
  channels: number
  descriptionBase64?: string
}

const audioBitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', media: 'audio', kbps })
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
const groupOrder = new GroupOrderGate((_groupId, object) => enqueueDirectDecode(object))

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

type AudioWorkerMessage =
  | SubgroupWorkerMessage
  | { type: 'catalog'; codec?: string; sampleRate?: number; channels?: number; descriptionBase64?: string }

self.onmessage = (event: MessageEvent<AudioWorkerMessage>) => {
  const data = event.data
  switch (data.type) {
    case 'catalog':
      catalogAudioCodec = data.codec || catalogAudioCodec
      catalogAudioSampleRate = data.sampleRate ?? catalogAudioSampleRate
      catalogAudioChannels = data.channels ?? catalogAudioChannels
      catalogAudioDescriptionBase64 = data.descriptionBase64 || catalogAudioDescriptionBase64
      return
    case 'object':
      audioBitrateLogger.addBytes(data.subgroupStreamObject.objectPayloadLength)
      groupOrder.push(data.groupId, data.subgroupStreamObject)
  }
}

function enqueueDirectDecode(object: SubgroupObjectWithLoc): void {
  if (!object.objectPayloadLength) {
    return
  }
  directDecodeQueue = directDecodeQueue
    .then(async () => {
      const loc = readLocHeader(object.locHeader)
      await decode(buildAudioChunkFromLoc(loc, object.objectPayload), loc.captureTimestampMicros)
    })
    .catch((error) => {
      console.error('[audioDecoder] direct decode failed', error)
    })
}

async function decode(decoded: DeserializedChunk, captureTimestampMicros?: number) {
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
  const description =
    resolved.codec.startsWith('mp4a') && resolved.descriptionBase64
      ? base64ToUint8Array(resolved.descriptionBase64)
      : undefined
  return {
    codec: resolved.codec,
    sampleRate: resolved.sampleRate,
    numberOfChannels: resolved.channels,
    description
  }
}

function buildSignature(resolved: CachedAudioConfig): string {
  const desc = resolved.descriptionBase64 ?? ''
  return `${resolved.codec}:${resolved.sampleRate}:${resolved.channels}:${desc}`
}
