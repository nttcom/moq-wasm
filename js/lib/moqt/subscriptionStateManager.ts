import { SubgroupStreamObjectMessage } from '../../pkg/moqt_client_wasm'

export type SubgroupStreamObjectMessageWithLoc = SubgroupStreamObjectMessage & { locHeader?: any }
export type SubgroupObjectHandler = (groupId: bigint, message: SubgroupStreamObjectMessageWithLoc) => void

export interface SubscriptionStateStore {
  setSubgroupObjectHandler(trackAlias: bigint, handler: SubgroupObjectHandler): void
  getSubgroupObjectHandler(trackAlias: bigint): SubgroupObjectHandler | undefined
  deleteSubgroupObjectHandler(trackAlias: bigint): void
  clearHandlers(): void
}

export class InMemorySubscriptionStateManager implements SubscriptionStateStore {
  private readonly subgroupObjectHandlers = new Map<bigint, SubgroupObjectHandler>()

  setSubgroupObjectHandler(trackAlias: bigint, handler: SubgroupObjectHandler): void {
    this.subgroupObjectHandlers.set(trackAlias, handler)
  }

  getSubgroupObjectHandler(trackAlias: bigint): SubgroupObjectHandler | undefined {
    return this.subgroupObjectHandlers.get(trackAlias)
  }

  deleteSubgroupObjectHandler(trackAlias: bigint): void {
    this.subgroupObjectHandlers.delete(trackAlias)
  }

  clearHandlers(): void {
    this.subgroupObjectHandlers.clear()
  }
}
