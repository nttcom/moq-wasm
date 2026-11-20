/// Where the incoming rendition takes over. `aligned` is false when its group
/// ids do not continue the outgoing ones, so the decoder has to start over.
export type SpliceStart = { groupId: bigint; aligned: boolean }

export type IncomingVerdict = {
  play: boolean
  /// Set by the object that decided where the incoming rendition starts.
  start?: SpliceStart
}

/// `overrun` is an object of a group past the start group. A publisher that
/// ended the outgoing subscription before the start sends none, so an overrun
/// shows it did not.
export type OutgoingVerdict = 'play' | 'drop' | 'overrun'

/// Hands live video over from one rendition to another of its switching set
/// at a group boundary. Renditions of one `altGroup` number their groups by
/// media time (draft-ietf-moq-msf-02 §4.2), so the outgoing one plays every
/// group before the first group the incoming one delivers from its start and
/// the decoder sees one continuous track. A first incoming group more than
/// one group away from the outgoing one means the ids are not aligned, and
/// the incoming rendition starts there at once.
export class RenditionSplice {
  private outgoingNewest: bigint | undefined
  private startGroupId: bigint | undefined
  incomingPlaying = false

  constructor(outgoingNewest: bigint | undefined) {
    this.outgoingNewest = outgoingNewest
  }

  get start(): bigint | undefined {
    return this.startGroupId
  }

  /// `drop` is an object of the incoming rendition's start group. A caller
  /// that asked the publisher to end the outgoing subscription there can
  /// still receive one sent before the publisher processed the request
  /// (draft-ietf-moq-transport-14 §9.10), and leaves the subscription to the
  /// publisher's PUBLISH_DONE; one that did not retires it.
  acceptOutgoing(groupId: bigint): OutgoingVerdict {
    if (this.startGroupId === undefined) {
      if (this.outgoingNewest === undefined || groupId > this.outgoingNewest) {
        this.outgoingNewest = groupId
      }
      return 'play'
    }
    if (groupId < this.startGroupId) {
      return 'play'
    }
    return groupId === this.startGroupId ? 'drop' : 'overrun'
  }

  acceptIncoming(groupId: bigint, objectId: bigint): IncomingVerdict {
    const verdict = this.judgeIncoming(groupId, objectId)
    this.incomingPlaying ||= verdict.play
    return verdict
  }

  private judgeIncoming(groupId: bigint, objectId: bigint): IncomingVerdict {
    if (this.startGroupId !== undefined) {
      return { play: groupId >= this.startGroupId }
    }
    if (objectId !== 0n) {
      return { play: false }
    }
    const newest = this.outgoingNewest
    const aligned = newest !== undefined && groupId >= newest && groupId <= newest + 1n
    const startGroupId = aligned ? newest + 1n : groupId
    this.startGroupId = startGroupId
    return {
      play: groupId >= startGroupId,
      start: { groupId: startGroupId, aligned }
    }
  }
}
