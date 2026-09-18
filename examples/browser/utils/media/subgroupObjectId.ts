import type { SubgroupObject } from './jitterBufferTypes'
import { isTerminalStatus } from './objectStatus'

export function trackSubgroupObjectId(
  lastObjectIds: Map<string, bigint>,
  groupId: bigint,
  object: SubgroupObject
): bigint {
  const key = `${groupId}:${object.subgroupId ?? 0n}`
  const previousObjectId = lastObjectIds.get(key)
  const objectId = previousObjectId === undefined ? object.objectIdDelta : previousObjectId + object.objectIdDelta + 1n
  if (isTerminalStatus(object.objectStatus)) {
    lastObjectIds.delete(key)
  } else {
    lastObjectIds.set(key, objectId)
  }
  return objectId
}
