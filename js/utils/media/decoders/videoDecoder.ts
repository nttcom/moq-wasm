import { VideoJitterBuffer } from '../videoJitterBuffer'
import type { JitterBufferSubgroupObject, SubgroupObject, SubgroupWorkerMessage } from '../jitterBufferTypes'
import { KEYFRAME_INTERVAL } from '../constants'
import { createBitrateLogger } from '../bitrate'

const bitrateLogger = createBitrateLogger((kbps) => {
  self.postMessage({ type: 'bitrate', kbps })
})

const KEYFRAME_INTERVAL_BIGINT = BigInt(KEYFRAME_INTERVAL)

const VIDEO_DECODER_CONFIG = {
  //codec: 'av01.0.08M.08',
  codec: 'avc1.640028',
  avc: {
    format: 'annexb'
  } as any,
  hardwareAcceleration: 'prefer-hardware' as any,
  width: 1920,
  height: 1080,
  scalabilityMode: 'L1T1'
}

let videoDecoder: VideoDecoder | undefined
async function initializeVideoDecoder() {
  function sendVideoFrameMessage(frame: VideoFrame): void {
    self.postMessage({ type: 'frame', frame })
    frame.close()
  }

  const init: VideoDecoderInit = {
    output: sendVideoFrameMessage,
    error: (e: any) => {
      console.log(e.message)
      videoDecoder = undefined
    }
  }
  const decoder = new VideoDecoder(init)
  decoder.configure(VIDEO_DECODER_CONFIG)
  return decoder
}

const POP_INTERVAL_MS = 5
const jitterBuffer = new VideoJitterBuffer(
  1800, // maxBufferSize
  'correctly', // mode
  KEYFRAME_INTERVAL_BIGINT // fallback keyframe interval
)
jitterBuffer.setMinDelay(2000) // playout delay to smooth playback (ms)

type DecodedState = {
  groupId: bigint
  objectId: bigint
}

let lastDecodedState: DecodedState | null = null
let previousGroupClosed = false

// objectIdの連続性をチェック（JitterBufferがcorrectlyモードの場合は冗長だが、念のため保持）
function checkObjectIdContinuity(currentGroupId: bigint, currentObjectId: bigint): void {
  // 初回はgroupId=0, objectId=0であることを確認（キーフレーム）
  if (!lastDecodedState) {
    if (currentGroupId !== 0n || currentObjectId !== 0n) {
      console.warn(
        `[Video] First frame must be groupId=0, objectId=0 (keyframe). Got: groupId=${currentGroupId}, objectId=${currentObjectId}`
      )
    }
    return
  }

  // groupIdが変わった場合: 前回のobjectIdが最後のdeltaframeかチェック
  if (currentGroupId !== lastDecodedState.groupId) {
    if (!previousGroupClosed) {
      const expectedLastObjectId = KEYFRAME_INTERVAL_BIGINT - 1n
      if (lastDecodedState.objectId !== expectedLastObjectId) {
        console.debug(
          `[Video] Group ended with unexpected objectId. Expected: ${expectedLastObjectId}, Got: ${lastDecodedState.objectId}, Group: ${lastDecodedState.groupId} -> ${currentGroupId}`
        )
      }
    }
    // 新しいgroupの最初のobjectIdは0であるべき
    if (currentObjectId !== 0n) {
      console.warn(
        `[Video] New group should start with objectId 0. Got: ${currentObjectId}, GroupId: ${currentGroupId}`
      )
    }
    return
  }

  // 同一group内での連続性チェック
  if (currentObjectId !== lastDecodedState.objectId + 1n) {
    console.warn(
      `[Video] Non-sequential objectId detected. Expected: ${lastDecodedState.objectId + 1n}, Got: ${currentObjectId}, Gap: ${
        currentObjectId - lastDecodedState.objectId - 1n
      }`
    )
  }
}

function recordDecodedFrame(groupId: bigint, objectId: bigint): void {
  previousGroupClosed = false
  lastDecodedState = { groupId, objectId }
}

function markGroupClosed(): void {
  previousGroupClosed = true
}

setInterval(() => {
  const entry = jitterBuffer.popWithMetadata()
  if (entry) {
    if (entry.isEndOfGroup) {
      markGroupClosed()
      return
    }
    decode(entry.groupId, entry.object)
  }
}, POP_INTERVAL_MS)

self.onmessage = async (event: MessageEvent<SubgroupWorkerMessage>) => {
  const subgroupStreamObject: SubgroupObject = {
    objectId: event.data.subgroupStreamObject.objectId,
    objectPayloadLength: event.data.subgroupStreamObject.objectPayloadLength,
    objectPayload: new Uint8Array(event.data.subgroupStreamObject.objectPayload),
    objectStatus: event.data.subgroupStreamObject.objectStatus
  }
  bitrateLogger.addBytes(subgroupStreamObject.objectPayloadLength)

  jitterBuffer.push(event.data.groupId, subgroupStreamObject.objectId, subgroupStreamObject)
}

async function decode(groupId: bigint, subgroupStreamObject: JitterBufferSubgroupObject) {
  // objectIdの連続性をチェック
  checkObjectIdContinuity(groupId, subgroupStreamObject.objectId)
  recordDecodedFrame(groupId, subgroupStreamObject.objectId)

  const decoded = subgroupStreamObject.cachedChunk
  reportLatency(decoded.metadata.sentAt, 'video')

  const encodedVideoChunk = new EncodedVideoChunk({
    type: decoded.metadata.type as EncodedVideoChunkType,
    timestamp: decoded.metadata.timestamp,
    duration: decoded.metadata.duration ?? undefined,
    data: decoded.data
  })

  if (!videoDecoder || videoDecoder.state === 'closed') {
    videoDecoder = await initializeVideoDecoder()
    // デコーダー再初期化後の最初のフレームはキーフレームである必要がある
    if (decoded.metadata.type !== 'key') {
      return
    }
  }

  await videoDecoder.decode(encodedVideoChunk)
}

function reportLatency(sentAt: number | undefined, media: 'video') {
  if (typeof sentAt !== 'number') {
    return
  }
  const latency = Date.now() - sentAt
  if (!Number.isFinite(latency) || latency < 0) {
    return
  }
  self.postMessage({ type: 'latency', media, ms: latency })
}
