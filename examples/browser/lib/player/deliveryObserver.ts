export type StreamKind = 'subscribe' | 'fetch'

/// Live playback follows a subscribe stream, review playback a FETCH stream;
/// each keeps its own playhead so a rewind does not move the live one.
export type Playhead = {
  kind: StreamKind
  trackAlias: bigint
  groupId: bigint
  objectId: bigint
  captureMicros: number
}

export interface DeliveryObserver {
  label(trackAlias: bigint, track: string): void
  object(
    trackAlias: bigint,
    groupId: bigint,
    objectId: bigint,
    payloadLength: number,
    endsGroup: boolean,
    now?: number,
    captureMicros?: number
  ): void
  /// One FETCH response is one stream however many groups it spans; the
  /// request id stands in for the group id in the row key.
  fetchObject(
    requestId: bigint,
    track: string,
    groupId: bigint,
    objectId: bigint,
    payloadLength: number,
    captureMicros?: number
  ): void
  fetchFinished(requestId: bigint): void
  forget(trackAlias: bigint): void
  setPlayhead(playhead: Playhead): void
  clearPlayhead(kind: StreamKind): void
}
