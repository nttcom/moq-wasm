import type { MOQTClient } from '../../pkg/moqt'
import { buildLocHeader } from '../../utils/media/loc'
import { OBJECT_STATUS_END_OF_GROUP } from '../../utils/media/objectStatus'

const SUBGROUP_ID = 0n
const PUBLISHER_PRIORITY = 0

export interface SubgroupObserver {
  opened(trackAlias: bigint, groupId: bigint): void
  object(
    trackAlias: bigint,
    groupId: bigint,
    objectId: bigint,
    payloadLength: number,
    endsGroup: boolean,
    now?: number,
    captureMicros?: number
  ): void
}

export class SubgroupWriter {
  private groupId: bigint | undefined
  private nextObjectId = 0n

  constructor(
    private readonly client: MOQTClient,
    private readonly trackAlias: bigint,
    private readonly observer?: SubgroupObserver
  ) {}

  get openGroupId(): bigint | undefined {
    return this.groupId
  }

  async openGroup(groupId: bigint): Promise<void> {
    await this.closeGroup()
    this.groupId = groupId
    this.nextObjectId = 0n
    await this.client.sendSubgroupHeader(this.trackAlias, groupId, SUBGROUP_ID, PUBLISHER_PRIORITY)
    this.observer?.opened(this.trackAlias, groupId)
  }

  async send(payload: Uint8Array, captureMicros?: number): Promise<void> {
    const groupId = this.groupId
    if (groupId === undefined) {
      throw new Error('send before openGroup')
    }
    const locHeader =
      captureMicros === undefined ? undefined : buildLocHeader({ captureTimestampMicros: captureMicros })
    await this.client.sendSubgroupObject(
      this.trackAlias,
      groupId,
      SUBGROUP_ID,
      this.nextObjectId,
      undefined,
      payload,
      locHeader
    )
    this.observer?.object(
      this.trackAlias,
      groupId,
      this.nextObjectId,
      payload.byteLength,
      false,
      undefined,
      captureMicros
    )
    this.nextObjectId += 1n
  }

  async closeGroup(): Promise<void> {
    const groupId = this.groupId
    if (groupId === undefined) {
      return
    }
    this.groupId = undefined
    await this.client.sendSubgroupObject(
      this.trackAlias,
      groupId,
      SUBGROUP_ID,
      this.nextObjectId,
      OBJECT_STATUS_END_OF_GROUP,
      new Uint8Array(0),
      undefined
    )
    this.observer?.object(this.trackAlias, groupId, this.nextObjectId, 0, true)
  }
}
