function unpackMetaAndChunk(payload: Uint8Array): { meta: any; chunkArray: Uint8Array } {
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength)
  const metaLen = view.getUint32(0)
  const metaBytes = payload.slice(4, 4 + metaLen)
  const metaJson = new TextDecoder().decode(metaBytes)
  const meta = JSON.parse(metaJson)
  const chunkArray = payload.slice(4 + metaLen)
  return { meta, chunkArray }
}
import { JitterBuffer } from './jitterBuffer'

function sendAudioDataMessage(audioData: AudioData): void {
  self.postMessage({ audioData })
  audioData.close()
}

let audioDecoder: AudioDecoder | undefined
async function initializeAudioDecoder() {
  const init: AudioDecoderInit = {
    output: sendAudioDataMessage,
    error: (e: any) => {
      console.log(e.message)
    }
  }
  const config = {
    codec: 'opus',
    sampleRate: 48000, // Opusの推奨サンプルレート
    numberOfChannels: 1, // モノラル
    bitrate: 64000 // 64kbpsのビットレート
  }
  const decoder = new AudioDecoder(init)
  decoder.configure(config)
  return decoder
}

namespace AudioDecoder {
  export type SubgroupStreamObject = {
    objectId: number
    objectPayloadLength: number
    objectPayload: Uint8Array
    objectStatus: any
  }
}

const POP_INTERVAL_MS = 5
const jitterBuffer: JitterBuffer<AudioDecoder.SubgroupStreamObject> = new JitterBuffer()

setInterval(() => {
  const subgroupStreamObject = jitterBuffer.pop()
  if (subgroupStreamObject) {
    decode(subgroupStreamObject)
  }
}, POP_INTERVAL_MS)

self.onmessage = async (event) => {
  const subgroupStreamObject: AudioDecoder.SubgroupStreamObject = {
    objectId: event.data.subgroupStreamObject.object_id,
    objectPayloadLength: event.data.subgroupStreamObject.object_payload_length,
    objectPayload: new Uint8Array(event.data.subgroupStreamObject.object_payload),
    objectStatus: event.data.subgroupStreamObject.object_status
  }

  jitterBuffer.push(event.data.groupId, subgroupStreamObject.objectId, subgroupStreamObject)
}

async function decode(subgroupStreamObject: AudioDecoder.SubgroupStreamObject) {
  const { meta, chunkArray } = unpackMetaAndChunk(subgroupStreamObject.objectPayload)

  const encodedAudioChunk = new EncodedAudioChunk({
    type: meta.type,
    timestamp: meta.timestamp,
    duration: meta.duration,
    data: chunkArray
  })

  if (!audioDecoder || audioDecoder.state === 'closed') {
    audioDecoder = await initializeAudioDecoder()
  }

  await audioDecoder.decode(encodedAudioChunk)
}
