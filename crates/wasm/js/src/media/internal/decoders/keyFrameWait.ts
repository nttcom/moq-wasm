/// Whether the video decoder takes delta frames, or drops them until a
/// keyframe. A wait that starts because the decoder lost the group it was
/// decoding asks the publisher once for a new group; the wait for the first
/// keyframe does not, since a subscription already starts at a group.
export class KeyFrameWait {
  private state: 'waiting' | 'decoding' | 'cut' = 'waiting'

  get waiting(): boolean {
    return this.state !== 'decoding'
  }

  /// The decoder can no longer decode the deltas of its current group.
  cut(): void {
    this.state = 'cut'
  }

  keyFrameDecoded(): void {
    this.state = 'decoding'
  }

  /// A delta was dropped while waiting. True for the first one after a cut:
  /// the caller asks for a new group then, and not again for the same wait.
  deltaDropped(): boolean {
    if (this.state !== 'cut') {
      return false
    }
    this.state = 'waiting'
    return true
  }
}
