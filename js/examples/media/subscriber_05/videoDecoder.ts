function unpackMetaAndChunk(payload: Uint8Array): { meta: any; chunkArray: Uint8Array } {
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength)
  const metaLen = view.getUint32(0)
  const metaBytes = payload.slice(4, 4 + metaLen)
  const metaJson = new TextDecoder().decode(metaBytes)
  const meta = JSON.parse(metaJson)
  const chunkArray = payload.slice(4 + metaLen)
  return { meta, chunkArray }
}
// 受信データのビットレート計測用
function createBitrateLogger() {
  let bytesThisSecond = 0
  let lastLogTime = performance.now()
  return {
    addBytes(byteLength: number) {
      bytesThisSecond += byteLength
      const now = performance.now()
      if (now - lastLogTime >= 1000) {
        const mbps = (bytesThisSecond * 8) / 1_000_000
        console.log(`Received bitrate: ${mbps.toFixed(2)} Mbps`)
        bytesThisSecond = 0
        lastLogTime = now
      }
    }
  }
}

const bitrateLogger = createBitrateLogger()
import { JitterBuffer } from './jitterBuffer'

// タイムスタンプを保持するためのキュー
const timestampQueue: number[] = [];

function sendVideoFrameMessage(frame: VideoFrame): void {
  const senderTimestamp = timestampQueue.shift(); // デコードが終わったらキューから取り出す
  self.postMessage({ frame, senderTimestamp });
  // frame.close() is called in ui.ts
}

let videoDecoder: VideoDecoder | undefined

// const HW_VIDEO_DECODER_CONFIG = {
//   // codec: 'av01.0.04M.08',
//   codec: 'avc1.640028',
//   avc: {
//     format: 'annexb'
//   } as any,
//   hardwareAcceleration: 'prefer-hardware' as any,
//   width: 1920,
//   height: 1080,
//   scalabilityMode: 'L1T1'
// }

const SW_VIDEO_DECODER_CONFIG = {
  codec: 'av01.0.08M.08',
  width: 1920,
  height: 1080,
  // scalabilityMode: 'L1T3'
  scalabilityMode: 'L1T1'
}
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
  await decoder.configure(SW_VIDEO_DECODER_CONFIG)
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

// JitterBufferに格納するペイロードの型定義
interface JitterBufferPayload {
  subgroupStreamObject: VideoDecoder.SubgroupStreamObject;
  senderTimestamp: number;
}

const POP_INTERVAL_MS = 5
const jitterBuffer: JitterBuffer<JitterBufferPayload> = new JitterBuffer()

let waitingForKeyframe = true;

setInterval(() => {
  const payload = jitterBuffer.pop()
  if (payload) {
    decode(payload.subgroupStreamObject, payload.senderTimestamp)
  }
}, POP_INTERVAL_MS)

self.onmessage = async (event) => {
  const size = event.data.subgroupStreamObject.objectPayloadLength
  bitrateLogger.addBytes(size)

  const subgroupStreamObject: VideoDecoder.SubgroupStreamObject = {
    objectId: event.data.subgroupStreamObject.objectId,
    objectPayloadLength: event.data.subgroupStreamObject.objectPayloadLength,
    objectPayload: new Uint8Array(event.data.subgroupStreamObject.objectPayload as ArrayBuffer),
    objectStatus: event.data.subgroupStreamObject.objectStatus
  }

  const payload: JitterBufferPayload = {
    subgroupStreamObject: subgroupStreamObject,
    senderTimestamp: event.data.senderTimestamp
  };

  jitterBuffer.push(event.data.groupId, subgroupStreamObject.objectId, payload)
}

async function decode(subgroupStreamObject: VideoDecoder.SubgroupStreamObject, senderTimestamp: number) {
  const { meta, chunkArray } = unpackMetaAndChunk(subgroupStreamObject.objectPayload)

  if (waitingForKeyframe && meta.type !== 'key') {
    console.log('Discarding delta frame while waiting for a key frame.');
    return;
  }

  if (meta.type === 'key') {
    waitingForKeyframe = false;
  }

  const encodedVideoChunk = new EncodedVideoChunk({
    type: meta.type,
    timestamp: meta.timestamp,
    duration: meta.duration,
    data: chunkArray
  })

  if (!videoDecoder || videoDecoder.state === 'closed') {
    console.log('initializeVideoDecoder')
    videoDecoder = await initializeVideoDecoder()
    // The first frame after initializing the decoder must be a keyframe
    if (meta.type !== 'key') {
      waitingForKeyframe = true; // Decoder was reset, wait for a new keyframe
      return
    }
  }

  timestampQueue.push(senderTimestamp); // デコードする前にキューに追加
  await videoDecoder.decode(encodedVideoChunk)
}
