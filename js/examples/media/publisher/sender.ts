import { MOQTClient } from '../../../pkg/moqt_client_sample'
import { createBitrateLogger } from '../../../utils/media/logger'

const chunkDataBitrateLogger = createBitrateLogger('chunkData bitrate')
const videoGroupStates = new Map<bigint, { groupId: bigint; lastObjectId: bigint }>()

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

  const chunkArray = new Uint8Array(chunk.byteLength)
  chunk.copyTo(chunkArray)

  const totalLen = 4 + metaLen + chunkArray.length
  const payload = new Uint8Array(totalLen)
  const view = new DataView(payload.buffer)
  view.setUint32(0, metaLen)
  payload.set(metaBytes, 4)
  payload.set(chunkArray, 4 + metaLen)
  return payload
}

export async function sendVideoObjectMessage(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedVideoChunk,
  client: MOQTClient
) {
  const previousState = videoGroupStates.get(trackAlias)
  if (previousState && previousState.groupId !== groupId) {
    const endObjectId = previousState.lastObjectId + 1n
    await client.sendSubgroupStreamObject(trackAlias, previousState.groupId, 0n, endObjectId, 3, new Uint8Array(0))
    console.log(
      `[MediaPublisher] Sent EndOfGroup trackAlias=${trackAlias} groupId=${previousState.groupId} subgroupId=0 objectId=${endObjectId}`
    )
  }

  chunkDataBitrateLogger.addBytes(chunk.byteLength)
  const payload = packMetaAndChunk(chunk)
  await client.sendSubgroupStreamObject(BigInt(trackAlias), groupId, subgroupId, objectId, undefined, payload)
  videoGroupStates.set(trackAlias, { groupId, lastObjectId: objectId })
}

export async function sendAudioObjectMessage(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedAudioChunk,
  client: MOQTClient
) {
  const payload = packMetaAndChunk(chunk)

  await client.sendSubgroupStreamObject(BigInt(trackAlias), groupId, subgroupId, objectId, undefined, payload)
}
