import { timeSeededGroupId } from './clock'

type SubgroupId = number
type TrackAlias = bigint | string

class TrackTransportState {
  groupId = timeSeededGroupId() - 1n
  nextObjectId = 0n
  private readonly headerSentAliases = new Map<SubgroupId, Set<string>>([[0, new Set()]])

  ensureSubgroup(subgroupId: SubgroupId): void {
    if (!this.headerSentAliases.has(subgroupId)) {
      this.headerSentAliases.set(subgroupId, new Set())
    }
  }

  listSubgroups(): SubgroupId[] {
    return Array.from(this.headerSentAliases.keys())
  }

  /// Moves to the next group id, or to `atLeast` when that is larger.
  advanceGroup(atLeast?: bigint): void {
    this.groupId += 1n
    if (atLeast !== undefined && atLeast > this.groupId) {
      this.groupId = atLeast
    }
    this.nextObjectId = 0n
    for (const aliases of this.headerSentAliases.values()) {
      aliases.clear()
    }
  }

  hasHeaderSent(trackAlias: TrackAlias, subgroupId: SubgroupId): boolean {
    return this.headerSentAliases.get(subgroupId)?.has(String(trackAlias)) ?? false
  }

  markHeaderSent(trackAlias: TrackAlias, subgroupId: SubgroupId): void {
    this.ensureSubgroup(subgroupId)
    this.headerSentAliases.get(subgroupId)!.add(String(trackAlias))
  }

  resetAlias(trackAlias: TrackAlias): void {
    for (const aliases of this.headerSentAliases.values()) {
      aliases.delete(String(trackAlias))
    }
  }
}

export class MediaTransportState {
  readonly video = new TrackTransportState()
  readonly audio = new TrackTransportState()
  private readonly videoCodecSent = new Set<string>()
  private readonly videoKeyframeDelivered = new Set<string>()

  shouldSendVideoCodec(trackAlias: TrackAlias): boolean {
    return !this.videoCodecSent.has(String(trackAlias))
  }

  markVideoCodecSent(trackAlias: TrackAlias): void {
    this.videoCodecSent.add(String(trackAlias))
  }

  hasVideoKeyframeDelivered(trackAlias: TrackAlias): boolean {
    return this.videoKeyframeDelivered.has(String(trackAlias))
  }

  markVideoKeyframeDelivered(trackAlias: TrackAlias): void {
    this.videoKeyframeDelivered.add(String(trackAlias))
  }

  /// A re-subscribing alias must receive the headers, the codec config and a
  /// keyframe again from scratch.
  resetAlias(trackAlias: TrackAlias): void {
    this.video.resetAlias(trackAlias)
    this.audio.resetAlias(trackAlias)
    this.videoCodecSent.delete(String(trackAlias))
    this.videoKeyframeDelivered.delete(String(trackAlias))
  }
}
