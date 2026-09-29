import { type FetchObjectToSend, GroupOrder, type IncomingFetchContext, RequestErrorCode } from '@moqt/moqtClient'

export type ReplayObject = Omit<FetchObjectToSend, 'groupId'>

export interface ReplayTrack {
  groupIds(): bigint[]
  objects(groupId: bigint): AsyncIterable<ReplayObject>
}

/// draft-ietf-moq-transport-14 §9.16.3: groups go out in the delivered group
/// order, objects in Object ID order within a group. The End Location has
/// already been clamped by the client: it is the Location after the last
/// object to send, where Object 0 covers the whole group.
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

export type SampleSpan = {
  passOriginMicros: number
  firstSample: number
  endSample: number
}

/// A group runs from its keyframe to the next one, which on a looped file
/// can lie in the next pass, so a group is recorded as the sample spans of
/// each pass it covers.
export class PublishedGroupLog {
  private readonly groups = new Map<bigint, SampleSpan[]>()
  private openGroupId: bigint | undefined

  constructor(private readonly sampleCount: number) {}

  startGroup(groupId: bigint, firstSample: number, passOriginMicros: number): void {
    const openSpans = this.openGroupId === undefined ? undefined : this.groups.get(this.openGroupId)
    const openSpan = openSpans?.[openSpans.length - 1]
    if (openSpan) {
      openSpan.endSample = firstSample
    }
    this.groups.set(groupId, [{ passOriginMicros, firstSample, endSample: this.sampleCount }])
    this.openGroupId = groupId
  }

  startPass(passOriginMicros: number): void {
    if (this.openGroupId !== undefined) {
      this.groups.get(this.openGroupId)?.push({ passOriginMicros, firstSample: 0, endSample: this.sampleCount })
    }
  }

  groupIds(): bigint[] {
    return Array.from(this.groups.keys())
  }

  spans(groupId: bigint): SampleSpan[] {
    return this.groups.get(groupId) ?? []
  }

  isClosed(groupId: bigint): boolean {
    return this.groups.has(groupId) && groupId !== this.openGroupId
  }
}
