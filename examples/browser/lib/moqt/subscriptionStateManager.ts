import { SubgroupObjectMessage } from '../../pkg/moqt_client_wasm'

export type SubgroupObjectMessageWithLoc = SubgroupObjectMessage & { locHeader?: any }
export type SubgroupObjectHandler = (groupId: bigint, message: SubgroupObjectMessageWithLoc) => void
type BufferedSubgroupObject = { groupId: bigint; message: SubgroupObjectMessageWithLoc }

export interface SubscriptionStateStore {
  setSubgroupObjectHandler(trackAlias: bigint, handler: SubgroupObjectHandler): void
  getSubgroupObjectHandler(trackAlias: bigint): SubgroupObjectHandler | undefined
  bufferSubgroupObject(trackAlias: bigint, groupId: bigint, message: SubgroupObjectMessageWithLoc): void
  deleteSubgroupObjectHandler(trackAlias: bigint): void
  clearHandlers(): void
}

export class InMemorySubscriptionStateManager implements SubscriptionStateStore {
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
    if (buffered.length >= InMemorySubscriptionStateManager.MAX_BUFFERED_OBJECTS_PER_TRACK) {
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
