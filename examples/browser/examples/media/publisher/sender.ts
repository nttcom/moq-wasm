import { MOQTClient } from '../../../pkg/moqt_client_wasm'
import { createBitrateLogger } from '../../../utils/media/logger'
import type { LocHeader } from '../../../utils/media/loc'

const chunkDataBitrateLogger = createBitrateLogger('chunkData bitrate')
const videoGroupStates = new Map<bigint, { groupId: bigint; lastObjectNumber: bigint }>()

async function sendVideoEndOfGroup(trackAlias: bigint, groupId: bigint, objectNumber: bigint, client: MOQTClient) {
  await client.sendSubgroupObject(trackAlias, groupId, 0n, objectNumber, 3, new Uint8Array(0), undefined)
  console.log(
    `[MediaPublisher] Sent EndOfGroup trackAlias=${trackAlias} groupId=${groupId} subgroupId=0 objectNumber=${objectNumber}`
  )
}

export async function sendVideoObjectMessage(
  trackAlias: bigint,
  groupId: bigint,
  subgroupId: bigint,
  objectNumber: bigint,
  payload: Uint8Array,
  client: MOQTClient,
  locHeader?: LocHeader
) {
  console.debug('[MediaPublisher] sendVideoObjectMessage', {
    trackAlias: trackAlias.toString(),
    groupId: groupId.toString(),
    subgroupId: subgroupId.toString(),
    objectNumber: objectNumber.toString(),
    byteLength: payload.byteLength
  })
  const previousState = videoGroupStates.get(trackAlias)
  if (previousState && previousState.groupId !== groupId) {
    const endObjectNumber = previousState.lastObjectNumber + 1n
    await sendVideoEndOfGroup(trackAlias, previousState.groupId, endObjectNumber, client)
  }

  chunkDataBitrateLogger.addBytes(payload.byteLength)
  await client.sendSubgroupObject(BigInt(trackAlias), groupId, subgroupId, objectNumber, undefined, payload, locHeader)
  videoGroupStates.set(trackAlias, { groupId, lastObjectNumber: objectNumber })
}
