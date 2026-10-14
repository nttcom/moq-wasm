/// Where the incoming rendition takes over. `aligned` is false when its group
/// ids do not continue the outgoing ones, so the decoder has to start over.
export type SpliceStart = { groupId: bigint; aligned: boolean }

export type IncomingVerdict = {
  play: boolean
  /// Set by the object that decided where the incoming rendition starts.
  start?: SpliceStart
  /// The outgoing subscription has nothing left to deliver.
  retireOutgoing: boolean
}

/// Hands live video over from one rendition to another of its switching set
/// at a group boundary. Renditions of one `altGroup` number their groups by
/// media time (draft-ietf-moq-msf-01 §4.2), so the outgoing one plays every
/// group before the first group the incoming one delivers from its start and
/// the decoder sees one continuous track. A first incoming group more than
/// one group away from the outgoing one means the ids are not aligned, and
/// the incoming rendition starts there at once.
export class RenditionSplice {
  private outgoingNewest: bigint | undefined
  private startGroupId: bigint | undefined

  constructor(outgoingNewest: bigint | undefined) {
    this.outgoingNewest = outgoingNewest
  }

  get start(): bigint | undefined {
    return this.startGroupId
  }

  /// `retire` drops the object and ends the outgoing subscription: it has
  /// reached the incoming rendition's start.
  acceptOutgoing(groupId: bigint): 'play' | 'retire' {
    if (this.startGroupId === undefined) {
      if (this.outgoingNewest === undefined || groupId > this.outgoingNewest) {
        this.outgoingNewest = groupId
      }
      return 'play'
    }
    return groupId < this.startGroupId ? 'play' : 'retire'
  }

  acceptIncoming(groupId: bigint, objectId: bigint): IncomingVerdict {
    if (this.startGroupId !== undefined) {
      return { play: groupId >= this.startGroupId, retireOutgoing: groupId > this.startGroupId }
    }
    if (objectId !== 0n) {
      return { play: false, retireOutgoing: false }
    }
    const newest = this.outgoingNewest
    const aligned = newest !== undefined && groupId >= newest && groupId <= newest + 1n
    const startGroupId = aligned ? newest + 1n : groupId
    this.startGroupId = startGroupId
    return {
      play: groupId >= startGroupId,
      start: { groupId: startGroupId, aligned },
      retireOutgoing: !aligned
    }
  }
}
