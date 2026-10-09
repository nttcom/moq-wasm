import type { MOQTClient } from '../../pkg/moqt'
import { OBJECT_STATUS_END_OF_GROUP } from './objectStatus'

const SUBGROUP_ID = 0n
const PUBLISHER_PRIORITY = 0

export async function sendSingleObjectGroup(
  client: MOQTClient,
  trackAlias: bigint,
  groupId: bigint,
  payload: Uint8Array
): Promise<void> {
  await client.sendSubgroupHeader(trackAlias, groupId, SUBGROUP_ID, PUBLISHER_PRIORITY)
  await client.sendSubgroupObject(trackAlias, groupId, SUBGROUP_ID, 0n, undefined, payload, undefined)
  await client.sendSubgroupObject(
    trackAlias,
    groupId,
    SUBGROUP_ID,
    1n,
    OBJECT_STATUS_END_OF_GROUP,
    new Uint8Array(0),
    undefined
  )
}
