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

self.onmessage = async (event) => {
  if (!audioDecoder) {
    audioDecoder = await initializeAudioDecoder()
  }

  const subgroupStreamObject: AudioDecoder.SubgroupStreamObject = {
    objectId: event.data.subgroupStreamObject.object_id,
    objectPayloadLength: event.data.subgroupStreamObject.object_payload_length,
    objectPayload: event.data.subgroupStreamObject.object_payload,
    objectStatus: event.data.subgroupStreamObject.object_status
  }
  // Rustから渡された時点ではUint8ArrayではなくArrayなので変換が必要
  const chunkArray = new Uint8Array(subgroupStreamObject.objectPayload)
  const decoder = new TextDecoder()
  const jsonString = decoder.decode(chunkArray)
  const objectPayload = JSON.parse(jsonString)

  const encodedAudioChunk = new EncodedAudioChunk({
    type: objectPayload.chunk.type,
    timestamp: objectPayload.chunk.timestamp,
    duration: objectPayload.chunk.duration,
    data: new Uint8Array(objectPayload.chunk.data)
  })

  await audioDecoder.decode(encodedAudioChunk)
}
