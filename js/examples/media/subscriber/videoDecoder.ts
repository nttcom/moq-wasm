function sendVideoFrameMessage(frame: VideoFrame): void {
  self.postMessage({ frame })
  frame.close()
}

let videoDecoder: VideoDecoder | undefined
async function initializeVideoDecoder() {
  const init: VideoDecoderInit = {
    output: sendVideoFrameMessage,
    error: (e: any) => {
      console.log(e.message)
      videoDecoder = undefined
    }
  }
  const config = {
    codec: 'av01.0.04M.08',
    width: 1920,
    height: 1080,
    scalabilityMode: 'L1T3'
  }
  const decoder = new VideoDecoder(init)
  decoder.configure(config)
  return decoder
}

namespace VideoDecoder {
  export type SubgroupStreamObject = {
    objectId: number
    objectPayloadLength: number
    objectPayload: Uint8Array
    objectStatus: any
  }
}

let keyframeDecoded = false
self.onmessage = async (event) => {
  const subgroupStreamObject: VideoDecoder.SubgroupStreamObject = {
    objectId: event.data.subgroupStreamObject.object_id,
    objectPayloadLength: event.data.subgroupStreamObject.object_payload_length,
    objectPayload: event.data.subgroupStreamObject.object_payload,
    objectStatus: event.data.subgroupStreamObject.object_status
  }

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

  if (!videoDecoder) {
    videoDecoder = await initializeVideoDecoder()
    // The first frame after initializing the decoder must be a keyframe
    if (objectPayload.chunk.type !== 'key') {
      return
    }
  }

  await videoDecoder.decode(encodedVideoChunk)
}
