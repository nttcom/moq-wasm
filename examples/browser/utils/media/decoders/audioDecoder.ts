import { GroupOrderGate } from '../groupOrderGate'
import { base64ToUint8Array } from '../base64'
import type { SubgroupObjectWithLoc, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { createBitrateLogger } from '../bitrate'
import { readLocHeader } from '../loc'

type AudioConfig = {
  codec: string
  sampleRate: number
  channels: number
  descriptionBase64?: string
}

const audioBitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', media: 'audio', kbps })
})

const audioConfig: AudioConfig = { codec: 'opus', sampleRate: 48000, channels: 1 }
let audioDecoder: AudioDecoder | undefined
/// The decoder stamps its outputs from the sample count it has produced, not
/// from the chunk timestamps, so a hole in the source would shift every later
/// output; each output is labelled with the capture timestamp of the chunk it
/// was decoded from instead.
const pendingCaptureTimestamps: (number | undefined)[] = []
let decoderSignature: string | null = null
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
      audioConfig.codec = data.codec || audioConfig.codec
      audioConfig.sampleRate = data.sampleRate ?? audioConfig.sampleRate
      audioConfig.channels = data.channels ?? audioConfig.channels
      audioConfig.descriptionBase64 = data.descriptionBase64 || audioConfig.descriptionBase64
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
    .then(() => decode(object.objectPayload, readLocHeader(object.locHeader).captureTimestampMicros))
    .catch((error) => {
      console.error('[audioDecoder] direct decode failed', error)
    })
}

async function decode(payload: Uint8Array, captureTimestampMicros: number | undefined) {
  const timestamp = captureTimestampMicros ?? 0
  if (isPcmAlawCodec(audioConfig.codec)) {
    try {
      decodePcmAlawChunk(payload, timestamp, captureTimestampMicros)
      if (audioDecoder && audioDecoder.state !== 'closed') {
        audioDecoder.close()
      }
      audioDecoder = undefined
    } catch (error) {
      console.error('[audioDecoder] pcm alaw decode failed', error)
    }
    return
  }

  const desiredSignature = buildSignature(audioConfig)
  if (!audioDecoder || audioDecoder.state === 'closed' || decoderSignature !== desiredSignature) {
    try {
      if (audioDecoder && audioDecoder.state !== 'closed') {
        audioDecoder.close()
      }
      audioDecoder = await createAudioDecoder(buildDecoderConfig(audioConfig), desiredSignature)
    } catch (e) {
      console.error('[audioDecoder] configure failed', e)
      return
    }
  }

  audioDecoder.decode(new EncodedAudioChunk({ type: 'key', timestamp, data: payload }))
  pendingCaptureTimestamps.push(captureTimestampMicros)
}

function decodePcmAlawChunk(payload: Uint8Array, timestamp: number, captureTimestampMicros: number | undefined): void {
  const channels = Math.max(1, audioConfig.channels)
  const sampleRate = Math.max(1, audioConfig.sampleRate)
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
    timestamp,
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

function buildDecoderConfig(config: AudioConfig): AudioDecoderConfig {
  const description =
    config.codec.startsWith('mp4a') && config.descriptionBase64
      ? base64ToUint8Array(config.descriptionBase64)
      : undefined
  return {
    codec: config.codec,
    sampleRate: config.sampleRate,
    numberOfChannels: config.channels,
    description
  }
}

function buildSignature(config: AudioConfig): string {
  return `${config.codec}:${config.sampleRate}:${config.channels}:${config.descriptionBase64 ?? ''}`
}
