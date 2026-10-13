import { SubgroupObjectMessage } from '../../pkg/moqt'

export type SubgroupObjectMessageWithLoc = SubgroupObjectMessage & { locHeader?: any }
export type SubgroupObjectHandler = (groupId: bigint, message: SubgroupObjectMessageWithLoc) => void
type BufferedSubgroupObject = { groupId: bigint; message: SubgroupObjectMessageWithLoc }

export class SubscriptionStateManager {
  private static readonly MAX_BUFFERED_OBJECTS_PER_TRACK = 32
  private readonly subgroupObjectHandlers = new Map<bigint, SubgroupObjectHandler>()
  private readonly bufferedSubgroupObjects = new Map<bigint, BufferedSubgroupObject[]>()

  setSubgroupObjectHandler(trackAlias: bigint, handler: SubgroupObjectHandler): void {
    this.subgroupObjectHandlers.set(trackAlias, handler)
    const buffered = this.bufferedSubgroupObjects.get(trackAlias)
    if (!buffered?.length) {
      return
    }
    this.bufferedSubgroupObjects.delete(trackAlias)
    for (const { groupId, message } of buffered) {
      handler(groupId, message)
    }
  }

  getSubgroupObjectHandler(trackAlias: bigint): SubgroupObjectHandler | undefined {
    return this.subgroupObjectHandlers.get(trackAlias)
  }

  bufferSubgroupObject(trackAlias: bigint, groupId: bigint, message: SubgroupObjectMessageWithLoc): void {
    const buffered = this.bufferedSubgroupObjects.get(trackAlias) ?? []
    if (buffered.length >= SubscriptionStateManager.MAX_BUFFERED_OBJECTS_PER_TRACK) {
      buffered.shift()
      console.warn('[moqt] dropping oldest buffered subgroup object', {
        trackAlias: trackAlias.toString()
      })
    }
    buffered.push({ groupId, message })
    this.bufferedSubgroupObjects.set(trackAlias, buffered)
  }

  deleteSubgroupObjectHandler(trackAlias: bigint): void {
    this.subgroupObjectHandlers.delete(trackAlias)
    this.bufferedSubgroupObjects.delete(trackAlias)
  }

  clearHandlers(): void {
    this.subgroupObjectHandlers.clear()
    this.bufferedSubgroupObjects.clear()
  }
}
