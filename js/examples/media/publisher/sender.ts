import { MOQTClient } from '../../../pkg/moqt_client_sample'
import { createBitrateLogger } from '../../../utils/media/logger'
import { serializeChunk, type ChunkMetadata } from '../../../utils/media/chunk'

const chunkDataBitrateLogger = createBitrateLogger('chunkData bitrate')
const videoGroupStates = new Map<bigint, { groupId: bigint; lastObjectId: bigint }>()

export async function sendVideoObjectMessage(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedVideoChunk,
  client: MOQTClient,
  extraMetadata?: Partial<ChunkMetadata>
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
  const payload = serializeChunk(
    {
      type: chunk.type,
      timestamp: chunk.timestamp,
      duration: chunk.duration ?? null,
      byteLength: chunk.byteLength,
      copyTo: (dest: Uint8Array) => chunk.copyTo(dest)
    },
    extraMetadata
  )
  await client.sendSubgroupStreamObject(BigInt(trackAlias), groupId, subgroupId, objectId, undefined, payload)
  videoGroupStates.set(trackAlias, { groupId, lastObjectId: objectId })
}

export async function sendAudioObjectMessage(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectId: bigint,
  chunk: EncodedAudioChunk,
  client: MOQTClient,
  extraMetadata?: Partial<ChunkMetadata>
) {
  const payload = serializeChunk(
    {
      type: chunk.type,
      timestamp: chunk.timestamp,
      duration: chunk.duration ?? null,
      byteLength: chunk.byteLength,
      copyTo: (dest: Uint8Array) => chunk.copyTo(dest)
    },
    extraMetadata
  )

  await client.sendSubgroupStreamObject(BigInt(trackAlias), groupId, subgroupId, objectId, undefined, payload)
}
