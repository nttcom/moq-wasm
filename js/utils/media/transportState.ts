type SubgroupId = number

interface SubgroupHeaderState {
  sentAliases: Set<string>
}

interface TrackCounters {
  groupId: bigint
  objectId: bigint
  subgroups: Map<SubgroupId, SubgroupHeaderState>
}

function normalizeAlias(alias: bigint | string): bigint {
  if (typeof alias === 'bigint') {
    return alias
  }
  try {
    return BigInt(alias)
  } catch {
    throw new Error(`Invalid alias value: ${alias}`)
  }
}

function aliasKey(alias: bigint | string): string {
  return normalizeAlias(alias).toString()
}

export class MediaTransportState {
  private readonly video: TrackCounters = {
    groupId: -1n,
    objectId: 0n,
    subgroups: new Map([[0, { sentAliases: new Set<string>() }]])
  }

  private readonly audio: TrackCounters = {
    groupId: 0n,
    objectId: 0n,
    subgroups: new Map([[0, { sentAliases: new Set<string>() }]])
  }

  private readonly audioCodecSent = new Set<string>()
  private readonly videoCodecSent = new Set<string>()

  ensureVideoSubgroup(subgroupId: SubgroupId): void {
    this.ensureSubgroup(this.video, subgroupId)
  }

  ensureAudioSubgroup(subgroupId: SubgroupId): void {
    this.ensureSubgroup(this.audio, subgroupId)
  }

  advanceVideoGroup(): void {
    this.video.groupId += 1n
    this.video.objectId = 0n
    this.resetHeaders(this.video)
  }

  getVideoGroupId(): bigint {
    return this.video.groupId
  }

  getVideoObjectId(): bigint {
    return this.video.objectId
  }

  incrementVideoObject(): void {
    this.video.objectId += 1n
  }

  hasVideoHeaderSent(trackAlias: bigint, subgroupId: SubgroupId): boolean {
    return this.hasHeaderSent(this.video, trackAlias, subgroupId)
  }

  markVideoHeaderSent(trackAlias: bigint, subgroupId: SubgroupId): void {
    this.markHeaderSent(this.video, trackAlias, subgroupId)
  }

  getAudioGroupId(): bigint {
    return this.audio.groupId
  }

  getAudioObjectId(): bigint {
    return this.audio.objectId
  }

  incrementAudioObject(): void {
    this.audio.objectId += 1n
  }

  shouldSendAudioHeader(trackAlias: bigint, subgroupId: SubgroupId): boolean {
    return !this.hasHeaderSent(this.audio, trackAlias, subgroupId)
  }

  markAudioHeaderSent(trackAlias: bigint, subgroupId: SubgroupId): void {
    this.markHeaderSent(this.audio, trackAlias, subgroupId)
  }

  shouldSendAudioCodec(trackAlias: bigint): boolean {
    return !this.audioCodecSent.has(aliasKey(trackAlias))
  }

  markAudioCodecSent(trackAlias: bigint): void {
    this.audioCodecSent.add(aliasKey(trackAlias))
  }

  listVideoSubgroups(): SubgroupId[] {
    return Array.from(this.video.subgroups.keys())
  }

  shouldSendVideoCodec(trackAlias: bigint): boolean {
    return !this.videoCodecSent.has(aliasKey(trackAlias))
  }

  markVideoCodecSent(trackAlias: bigint): void {
    this.videoCodecSent.add(aliasKey(trackAlias))
  }

  resetAlias(trackAlias: bigint | string): void {
    const key = aliasKey(trackAlias)
    for (const subgroup of this.video.subgroups.values()) {
      subgroup.sentAliases.delete(key)
    }
    for (const subgroup of this.audio.subgroups.values()) {
      subgroup.sentAliases.delete(key)
    }
    this.audioCodecSent.delete(key)
    this.videoCodecSent.delete(key)
  }

  private ensureSubgroup(state: TrackCounters, subgroupId: SubgroupId): void {
    if (!state.subgroups.has(subgroupId)) {
      state.subgroups.set(subgroupId, { sentAliases: new Set<string>() })
    }
  }

  private resetHeaders(state: TrackCounters): void {
    for (const subgroup of state.subgroups.values()) {
      subgroup.sentAliases.clear()
    }
  }

  private hasHeaderSent(state: TrackCounters, trackAlias: bigint | string, subgroupId: SubgroupId): boolean {
    this.ensureSubgroup(state, subgroupId)
    return state.subgroups.get(subgroupId)!.sentAliases.has(aliasKey(trackAlias))
  }

  private markHeaderSent(state: TrackCounters, trackAlias: bigint | string, subgroupId: SubgroupId): void {
    this.ensureSubgroup(state, subgroupId)
    state.subgroups.get(subgroupId)!.sentAliases.add(aliasKey(trackAlias))
  }
}
