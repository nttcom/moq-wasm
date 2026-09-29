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
