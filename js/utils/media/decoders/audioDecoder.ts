import { AudioJitterBuffer } from '../audioJitterBuffer'
import type { SubgroupObjectWithLoc, JitterBufferSubgroupObject, SubgroupWorkerMessage } from '../jitterBufferTypes'
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
  console.info('[audioDecoder] (re)initializing decoder with config:', config)
  console.info('[audioDecoder] desiredSignature:', signature)
  return decoder
}

const POP_INTERVAL_MS = 5
const jitterBuffer = new AudioJitterBuffer(1800, 'ordered')

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

type AudioWorkerMessage = SubgroupWorkerMessage | { type: 'config'; config: { mode?: string } }

self.onmessage = async (event: MessageEvent<AudioWorkerMessage>) => {
  if ((event.data as { type?: string }).type === 'config') {
    const config = (event.data as { type: 'config'; config: { mode?: string } }).config
    if (config.mode === 'ordered' || config.mode === 'latest') {
      jitterBuffer.setMode(config.mode)
      console.info('[audioDecoder] Set jitter buffer mode:', config.mode)
    }
    return
  }

  const message = event.data as SubgroupWorkerMessage
  const locHeader = message.subgroupStreamObject.locHeader
  const locExtensions = locHeader?.extensions ?? []
  const hasCaptureTimestamp = locExtensions.some((ext) => ext.type === 'captureTimestamp')
  const hasAudioLevel = locExtensions.some((ext) => ext.type === 'audioLevel')
  console.debug('[audioDecoder] recv object', {
    groupId: message.groupId,
    objectId: message.subgroupStreamObject.objectId,
    payloadLength: message.subgroupStreamObject.objectPayloadLength,
    status: message.subgroupStreamObject.objectStatus,
    locExtensionCount: locExtensions.length,
    hasCaptureTimestamp,
    hasAudioLevel
  })
  const subgroupStreamObject: SubgroupObjectWithLoc = {
    objectId: message.subgroupStreamObject.objectId,
    objectPayloadLength: message.subgroupStreamObject.objectPayloadLength,
    objectPayload: new Uint8Array(message.subgroupStreamObject.objectPayload),
    objectStatus: message.subgroupStreamObject.objectStatus,
    locHeader: message.subgroupStreamObject.locHeader
  }
  audioBitrateLogger.addBytes(subgroupStreamObject.objectPayloadLength)

  jitterBuffer.push(message.groupId, subgroupStreamObject.objectId, subgroupStreamObject, (latencyMs) =>
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
      console.info('[audioDecoder] (re)initializing decoder with config:', desiredConfig)
      console.info('[audioDecoder] desiredSignature:', desiredSignature)
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
