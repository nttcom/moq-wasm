import {
  JitterBuffer,
  type SubgroupObject,
  type JitterBufferSubgroupObject,
  type SubgroupWorkerMessage
} from '../jitterBuffer'
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
const jitterBuffer = new JitterBuffer(1800, 'audio')
jitterBuffer.setMinDelay(50)

setInterval(() => {
  const subgroupStreamObject = jitterBuffer.pop()
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

  const encodedAudioChunk = new EncodedAudioChunk({
    type: decoded.metadata.type as EncodedAudioChunkType,
    timestamp: decoded.metadata.timestamp,
    duration: decoded.metadata.duration ?? undefined,
    data: decoded.data
  })

  if (!audioDecoder || audioDecoder.state === 'closed') {
    audioDecoder = await initializeAudioDecoder()
  }

  await audioDecoder.decode(encodedAudioChunk)
}

function reportAudioLatency(sentAt: number | undefined) {
  if (typeof sentAt !== 'number') {
    return
  }
  const latency = Date.now() - sentAt
  if (!Number.isFinite(latency) || latency < 0) {
    return
  }
  self.postMessage({ type: 'latency', media: 'audio', ms: latency })
}
