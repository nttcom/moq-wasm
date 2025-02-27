function sendVideoFrameMessage(frame: VideoFrame): void {
  console.log(frame)
  self.postMessage({ frame })
}

let videoDecoder: VideoDecoder | undefined
async function initializeVideoDecoder() {
  const init: VideoDecoderInit = {
    output: sendVideoFrameMessage,
    error: (e: any) => {
      console.log(e.message)
    }
  }
  const config = {
    codec: 'av01.0.04M.08',
    width: 640,
    height: 480
  }
  const decoder = new VideoDecoder(init)
  decoder.configure(config)
  return decoder
}

async function startDecode(encodedVideoChunk: EncodedVideoChunk) {
  if (!videoDecoder) {
    videoDecoder = await initializeVideoDecoder()
  }
  await videoDecoder.decode(encodedVideoChunk)
}

type SubgroupStreamObject = {
  objectId: number
  objectPayloadLength: number
  objectPayload: Uint8Array
  objectStatus: any
}

self.onmessage = async (event) => {
  const subgroupStreamObject: SubgroupStreamObject = {
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

  const encodedVideoChunk = new EncodedVideoChunk({
    type: objectPayload.chunk.type,
    timestamp: objectPayload.chunk.timestamp,
    duration: objectPayload.chunk.duration,
    data: new Uint8Array(objectPayload.chunk.data)
  })

  await startDecode(encodedVideoChunk)
}
