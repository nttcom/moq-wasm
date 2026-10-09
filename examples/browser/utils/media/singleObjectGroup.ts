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

/// The relay isolates a track whose publisher repeats a location, so a
/// publisher that comes back must not restart its group ids at zero.
export function firstGroupId(): bigint {
  return BigInt(Date.now()) * 1_000n
}

/// One group per value, because the bots read every group from its first object.
export class JsonGroupPublisher<T> {
  private nextGroupId = firstGroupId()
  latestGroupId: bigint | undefined

  constructor(
    private readonly client: MOQTClient,
    readonly requestId: bigint,
    private readonly trackAlias: bigint
  ) {}

  async send(value: T): Promise<void> {
    const groupId = this.nextGroupId++
    this.latestGroupId = groupId
    await sendSingleObjectGroup(this.client, this.trackAlias, groupId, new TextEncoder().encode(JSON.stringify(value)))
  }
}
