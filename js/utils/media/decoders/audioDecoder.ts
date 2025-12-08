import { AudioJitterBuffer } from '../audioJitterBuffer'
import type { SubgroupObject, JitterBufferSubgroupObject, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { createBitrateLogger } from '../bitrate'
import type { ChunkMetadata } from '../chunk'

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
let remoteTimestampBase: number | null = null
let decoderSignature: string | null = null
let cachedAudioConfig: CachedAudioConfig | null = null

async function createAudioDecoder(config: AudioDecoderConfig, signature: string) {
  function sendAudioDataMessage(audioData: AudioData): void {
    self.postMessage({ type: 'audioData', audioData })
    audioData.close()
  }

  const init: AudioDecoderInit = {
    output: sendAudioDataMessage,
    error: (e: any) => {
      console.log(e.message)
    }
  }
  const decoder = new AudioDecoder(init)
  decoder.configure(config)
  decoderSignature = signature
  console.log('[audioDecoder] (re)initializing decoder with config:', config)
  console.log('[audioDecoder] desiredSignature:', signature)
  return decoder
}

const POP_INTERVAL_MS = 5
const jitterBuffer = new AudioJitterBuffer(1800)

setInterval(() => {
  const jitterBufferEntry = jitterBuffer.pop()
  if (!jitterBufferEntry) {
    return
  }
  console.debug(jitterBufferEntry)
  const subgroupStreamObject = jitterBufferEntry?.object
  if (subgroupStreamObject) {
    decode(subgroupStreamObject)
  }
}, POP_INTERVAL_MS)

self.onmessage = async (event: MessageEvent<SubgroupWorkerMessage>) => {
  const subgroupStreamObject: SubgroupObject = {
    objectId: event.data.subgroupStreamObject.objectId,
    objectPayloadLength: event.data.subgroupStreamObject.objectPayloadLength,
    objectPayload: new Uint8Array(event.data.subgroupStreamObject.objectPayload),
    objectStatus: event.data.subgroupStreamObject.objectStatus
  }
  audioBitrateLogger.addBytes(subgroupStreamObject.objectPayloadLength)

  jitterBuffer.push(event.data.groupId, subgroupStreamObject.objectId, subgroupStreamObject, (latencyMs) =>
    postReceiveLatency(latencyMs)
  )
}

async function decode(subgroupStreamObject: JitterBufferSubgroupObject) {
  const decoded = subgroupStreamObject.cachedChunk
  reportAudioLatency(decoded.metadata.sentAt)

  const resolvedConfig = resolveAudioConfig(decoded.metadata)
  if (!resolvedConfig) {
    // メタデータからデコーダ設定を導けるまで待つ
    return
  }
  const desiredConfig = buildDecoderConfig(resolvedConfig)
  const desiredSignature = buildSignature(resolvedConfig)

  if (!audioDecoder || audioDecoder.state === 'closed' || decoderSignature !== desiredSignature) {
    try {
      console.log('[audioDecoder] (re)initializing decoder with config:', desiredConfig)
      console.log('[audioDecoder] desiredSignature:', desiredSignature)
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

function reportAudioLatency(sentAt: number) {
  postRenderingLatency(Date.now() - sentAt)
}

function postReceiveLatency(latencyMs: number) {
  if (latencyMs < 0) {
    return
  }
  self.postMessage({ type: 'receiveLatency', media: 'audio', ms: latencyMs })
}

function postRenderingLatency(latencyMs: number) {
  if (latencyMs < 0) {
    return
  }
  self.postMessage({ type: 'renderingLatency', media: 'audio', ms: latencyMs })
}

/**
 * Convert sender-side timestamps to a local timeline so the decoder can play them.
 */
function rebaseTimestamp(remoteTimestamp: number): number {
  if (remoteTimestampBase === null) {
    remoteTimestampBase = remoteTimestamp
  }

  let rebased = remoteTimestamp - remoteTimestampBase
  return rebased
}

function resolveAudioConfig(metadata: ChunkMetadata): CachedAudioConfig | null {
  const hasNewConfig = metadata.codec || metadata.descriptionBase64 || metadata.sampleRate || metadata.channels
  if (!hasNewConfig && !cachedAudioConfig) {
    return null
  }

  const codec =
    metadata.codec ??
    (metadata.descriptionBase64 ? 'mp4a.40.2' : undefined) ??
    cachedAudioConfig?.codec
  const sampleRate =
    metadata.sampleRate ??
    cachedAudioConfig?.sampleRate ??
    DEFAULT_AUDIO_DECODER_CONFIG.sampleRate
  const channels =
    metadata.channels ??
    cachedAudioConfig?.channels ??
    DEFAULT_AUDIO_DECODER_CONFIG.numberOfChannels
  const descriptionBase64 = metadata.descriptionBase64 ?? cachedAudioConfig?.descriptionBase64

  if (!codec) {
    return null
  }

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
