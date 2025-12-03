import { AudioJitterBuffer } from '../audioJitterBuffer'
import type { SubgroupObject, JitterBufferSubgroupObject, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { createBitrateLogger } from '../bitrate'

const audioBitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', media: 'audio', kbps })
})

const AUDIO_DECODER_CONFIG = {
  codec: 'opus',
  sampleRate: 48000, // Opusの推奨サンプルレート
  numberOfChannels: 1, // モノラル
  bitrate: 64000 // 64kbpsのビットレート
}

let audioDecoder: AudioDecoder | undefined
let remoteTimestampBase: number | null = null

async function initializeAudioDecoder() {
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
  decoder.configure(AUDIO_DECODER_CONFIG)
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

  jitterBuffer.push(event.data.groupId, subgroupStreamObject.objectId, subgroupStreamObject)
}

async function decode(subgroupStreamObject: JitterBufferSubgroupObject) {
  const decoded = subgroupStreamObject.cachedChunk
  const timeNow = Date.now()
  reportLatency(timeNow, decoded.metadata.sentAt, 'audio')
  reportJitter(timeNow, decoded.metadata.sentAt, 'audio')

  const rebasedTimestamp = rebaseTimestamp(decoded.metadata.timestamp)

  const encodedAudioChunk = new EncodedAudioChunk({
    type: decoded.metadata.type as EncodedAudioChunkType,
    timestamp: rebasedTimestamp,
    duration: decoded.metadata.duration ?? undefined,
    data: decoded.data
  })

  if (!audioDecoder || audioDecoder.state === 'closed') {
    audioDecoder = await initializeAudioDecoder()
  }

  await audioDecoder.decode(encodedAudioChunk)
}

type JitterCalcState = {
  lastSentAt: number | undefined
  lastReceivedAt: number | undefined
  currentJitter: number
}
const jitterCalcState: JitterCalcState = {
  lastSentAt: undefined,
  lastReceivedAt: undefined,
  currentJitter: 0
}

function reportLatency(timeNow: number | undefined, sentAt: number | undefined, media: 'audio') {
  if (typeof timeNow !== 'number' || typeof sentAt !== 'number') {
    return
  }
  const latency = timeNow - sentAt
  if (!Number.isFinite(latency) || latency < 0) {
    return
  }
  self.postMessage({ type: 'latency', media, ms: latency })
}

function reportJitter(timeNow: number | undefined, sentAt: number | undefined, media: 'audio') {
  if (typeof timeNow !== 'number' || typeof sentAt !== 'number') {
    return
  }
  const receivedAt = timeNow
  // Check if we have previous timestamps to calculate jitter
  if (typeof jitterCalcState.lastSentAt === 'number' && typeof jitterCalcState.lastReceivedAt === 'number') {
    const transitSend = sentAt - jitterCalcState.lastSentAt
    const transitRecv = receivedAt - jitterCalcState.lastReceivedAt
    const d = transitRecv - transitSend
    // RFC 3550 smoothing algorithm
    jitterCalcState.currentJitter = jitterCalcState.currentJitter + (Math.abs(d) - jitterCalcState.currentJitter) / 16
    self.postMessage({ type: 'jitter', media, ms: jitterCalcState.currentJitter })
  }
  // Update timestamps for the next calculation
  jitterCalcState.lastSentAt = sentAt
  jitterCalcState.lastReceivedAt = receivedAt
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
