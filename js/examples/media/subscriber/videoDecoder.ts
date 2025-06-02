import { JitterBuffer } from './jitterBuffer'

function sendVideoFrameMessage(frame: VideoFrame): void {
  self.postMessage({ frame })
  frame.close()
}

let videoDecoder: VideoDecoder | undefined

const HW_VIDEO_DECODER_CONFIG = {
  // codec: 'av01.0.04M.08',
  codec: 'avc1.640028',
  avc: {
    format: 'annexb'
  } as any,
  hardwareAcceleration: 'prefer-hardware' as any,
  width: 1920,
  height: 1080,
  scalabilityMode: 'L1T1'
}

// const SW_VIDEO_DECODER_CONFIG = {
//   codec: 'av01.0.08M.08',
//   width: 1920,
//   height: 1080,
//   scalabilityMode: 'L1T3'
// }
async function initializeVideoDecoder() {
  const init: VideoDecoderInit = {
    output: sendVideoFrameMessage,
    error: (e: any) => {
      console.log(e.message)
      videoDecoder = undefined
    }
  }

  // console.log('isDecoderConfig Supported', await VideoDecoder.isConfigSupported(HW_VIDEO_DECODER_CONFIG))

  const decoder = new VideoDecoder(init)
  await decoder.configure(HW_VIDEO_DECODER_CONFIG)
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

const POP_INTERVAL_MS = 15
const jitterBuffer: JitterBuffer<VideoDecoder.SubgroupStreamObject> = new JitterBuffer()

setInterval(() => {
  const subgroupStreamObject = jitterBuffer.pop()
  if (subgroupStreamObject) {
    decode(subgroupStreamObject)
  }
}, POP_INTERVAL_MS)

self.onmessage = async (event) => {
  const subgroupStreamObject: VideoDecoder.SubgroupStreamObject = {
    objectId: event.data.subgroupStreamObject.object_id,
    objectPayloadLength: event.data.subgroupStreamObject.object_payload_length,
    objectPayload: event.data.subgroupStreamObject.object_payload,
    objectStatus: event.data.subgroupStreamObject.object_status
  }

  jitterBuffer.push(event.data.groupId, subgroupStreamObject.objectId, subgroupStreamObject)
}

async function decode(subgroupStreamObject: VideoDecoder.SubgroupStreamObject) {
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
