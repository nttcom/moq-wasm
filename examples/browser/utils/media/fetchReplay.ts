import { type FetchObjectToSend, GroupOrder, type IncomingFetchContext, RequestErrorCode } from '@moqt/moqtClient'
import { OBJECT_STATUS_END_OF_GROUP } from './objectStatus'

export type ReplayObject = Omit<FetchObjectToSend, 'groupId'>

export interface ReplayTrack {
  groupIds(): bigint[]
  objects(groupId: bigint): AsyncIterable<ReplayObject>
}

/// draft-ietf-moq-transport-14 §9.16.3: groups go out in the delivered group
/// order, objects in Object ID order within a group.
export async function answerFetch(context: IncomingFetchContext, track: ReplayTrack): Promise<void> {
  const { fetch, cancelSignal, respondOk, respondError, sendObject, finish } = context
  const groupIds = track.groupIds().filter((groupId) => groupId >= fetch.startGroupId && groupId <= fetch.endGroupId)
  if (!groupIds.length) {
    await respondError(RequestErrorCode.NoObjects, 'no objects in the requested range')
    return
  }
  await respondOk()
  if (fetch.groupOrder === GroupOrder.Descending) {
    groupIds.reverse()
  }
  for (const groupId of groupIds) {
    for await (const object of track.objects(groupId)) {
      if (cancelSignal.aborted) {
        return
      }
      if (groupId === fetch.startGroupId && object.objectId < fetch.startObjectId) {
        continue
      }
      if (groupId === fetch.endGroupId && fetch.endObjectId !== 0n && object.objectId >= fetch.endObjectId) {
        break
      }
      await sendObject({ groupId, ...object })
    }
  }
  await finish()
}

export function replayObject(objectId: bigint, payload: Uint8Array, locHeader?: unknown): ReplayObject {
  return { objectId, subgroupId: 0n, publisherPriority: 0, payload, locHeader }
}

export function endOfGroupObject(objectId: bigint): ReplayObject {
  return { ...replayObject(objectId, new Uint8Array(0)), objectStatus: OBJECT_STATUS_END_OF_GROUP }
}

export function documentReplayTrack(
  documents: Map<bigint, Uint8Array>,
  isClosed: (groupId: bigint) => boolean
): ReplayTrack {
  return {
    groupIds: () => Array.from(documents.keys()),
    async *objects(groupId: bigint) {
      const document = documents.get(groupId)
      if (!document) {
        return
      }
      yield replayObject(0n, document)
      if (isClosed(groupId)) {
        yield endOfGroupObject(1n)
      }
    }
  }
}
