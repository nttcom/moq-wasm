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
  reportAudioLatency(decoded.metadata.sentAt)

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

function reportAudioLatency(sentAt: number) {
  const latency = Date.now() - sentAt
  self.postMessage({ type: 'latency', media: 'audio', ms: latency })
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
