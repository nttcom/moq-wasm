import { postMetrics } from '../subscriber_03/metrics'

function packMetaAndChunk(chunk: {
  type: string
  timestamp: number
  duration: number | null
  byteLength: number
  copyTo: (dest: Uint8Array) => void
}): Uint8Array {
  const meta = { type: chunk.type, timestamp: chunk.timestamp, duration: chunk.duration ?? 0 }
  const metaJson = JSON.stringify(meta)
  const metaBytes = new TextEncoder().encode(metaJson)
  const metaLen = metaBytes.length

  // chunkデータをUint8Arrayに変換
  const chunkArray = new Uint8Array(chunk.byteLength)
  chunk.copyTo(chunkArray)

  const totalLen = 4 + metaLen + chunkArray.length
  const payload = new Uint8Array(totalLen)
  const view = new DataView(payload.buffer)
  view.setUint32(0, metaLen) // 先頭4バイトにメタデータ長
  payload.set(metaBytes, 4)
  payload.set(chunkArray, 4 + metaLen)
  return payload
}
// chunkDataのビットレート計測用
function createChunkDataBitrateLogger() {
  let bytesThisSecond = 0
  let lastLogTime = performance.now()
  return {
    addBytes(byteLength: number) {
      bytesThisSecond += byteLength
      const now = performance.now()
      if (now - lastLogTime >= 1000) {
        const mbps = (bytesThisSecond * 8) / 1_000_000
        console.log(`chunkData bitrate: ${mbps.toFixed(2)} Mbps`)
        postMetrics(  'pub', 'bitrate',  mbps.toFixed(2) * 1000 )
        bytesThisSecond = 0
        lastLogTime = now
      }
    }
  }
}

const chunkDataBitrateLogger = createChunkDataBitrateLogger()
import { MOQTClient } from '../../../pkg/moqt_client_sample'
import { KEYFRAME_INTERVAL } from './const'

let currentVideoId = {
  groupId: BigInt(0),
  subgroupId: BigInt(0),
  objectId: BigInt(0)
}

export async function sendVideoObjectMessage(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedVideoChunk,
  client: MOQTClient
) {
  chunkDataBitrateLogger.addBytes(chunk.byteLength)
  const payload = packMetaAndChunk(chunk)
  await client.sendSubgroupStreamObject(
    BigInt(trackAlias),
    groupId,
    subgroupId,
    objectId,
    undefined,
    payload
  )

  // groupIdが変わったら、EndOfGroupを送信
  // subgroupIdはなんでも良いが0とする
  if (groupId > currentVideoId.groupId) {
    // Do not send EndOfGroup for group 0, as it seems to cause issues at the start.
    if (currentVideoId.groupId > 0) {
      try {
        await client.sendSubgroupStreamObject(
          BigInt(trackAlias),
          currentVideoId.groupId,
          BigInt(0),
          BigInt(KEYFRAME_INTERVAL),
          3, // 0x3: EndOfGroup
          Uint8Array.from([])
        )
        console.log('send Object(ObjectStatus=EndOfGroup)')
      } catch (e) {
        console.warn('Failed to send EndOfGroup message:', e)
      }
    }
  }

  currentVideoId = { groupId, subgroupId, objectId }
}

export async function sendAudioObjectMessage(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedAudioChunk,
  client: MOQTClient
) {
  // `EncodedAudioChunk` のデータを Uint8Array に変換
  const payload = packMetaAndChunk(chunk)

  await client.sendSubgroupStreamObject(
    BigInt(trackAlias),
    groupId,
    subgroupId,
    objectId,
    undefined,
    payload
  )
}