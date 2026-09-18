import { isTerminalStatus } from './objectStatus'

export function normalizeSubgroupId(subgroupId: bigint | undefined): bigint {
  return subgroupId ?? 0n
}

function makeSubgroupKey(groupId: bigint, subgroupId: bigint): string {
  return `${groupId.toString()}:${subgroupId.toString()}`
}

function assignSubgroupObjectId(
  lastObjectIds: Map<string, bigint>,
  groupId: bigint,
  subgroupId: bigint,
  objectIdDelta: bigint
): bigint {
  const key = makeSubgroupKey(groupId, subgroupId)
  const previousObjectId = lastObjectIds.get(key)
  const objectId = previousObjectId === undefined ? objectIdDelta : previousObjectId + objectIdDelta + 1n
  lastObjectIds.set(key, objectId)
  return objectId
}

export function trackSubgroupObjectId(
  lastObjectIds: Map<string, bigint>,
  groupId: bigint,
  subgroupId: bigint | undefined,
  objectIdDelta: bigint,
  objectStatus: number | undefined
): bigint {
  const normalizedSubgroupId = normalizeSubgroupId(subgroupId)
  const objectId = assignSubgroupObjectId(lastObjectIds, groupId, normalizedSubgroupId, objectIdDelta)
  if (isTerminalStatus(objectStatus)) {
    lastObjectIds.delete(makeSubgroupKey(groupId, normalizedSubgroupId))
  }
  return objectId
}
