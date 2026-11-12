import type { PublishDone } from '../../core/session'
import { type MediaCatalogTrack, catalogTrackNamespace } from '../catalog'
import { subscribeAddedTrack } from './addedTrack'
import { AudioLane } from './audioLane'
import type { LivePlayout } from './livePlayout'
import type { TrackContext } from './trackContext'

export type AddedAudioTrackListener = {
  /// Once: the first chunk that arrived in time is scheduled for `dueMs` on
  /// the page clock (`performance.now()`). The track has caught up with the
  /// picture from then on, so raising its volume at `dueMs` loses nothing.
  onFirstScheduled?(dueMs: number): void
  /// The publisher ended the track with PUBLISH_DONE.
  onEnded?(done: PublishDone): void
}

/// An audio track `LivePlayer.addAudioTrack` plays on the live clock next to
/// the selected one, such as a translation of it. It starts silent.
export class AddedAudioTrack {
  private received = 0
  private unsubscribe: () => Promise<void> = async () => {}

  private constructor(
    private readonly lane: AudioLane,
    private readonly onClosed: () => void
  ) {}

  static async open(
    context: TrackContext,
    playout: LivePlayout,
    track: MediaCatalogTrack,
    listener: AddedAudioTrackListener,
    onClosed: () => void
  ): Promise<AddedAudioTrack> {
    const lane = new AudioLane(track, playout, (dueMs) => listener.onFirstScheduled?.(dueMs))
    const added = new AddedAudioTrack(lane, onClosed)
    try {
      added.unsubscribe = await subscribeAddedTrack(
        context,
        catalogTrackNamespace(track, context.namespace),
        track.name,
        (groupId, object, cached) => {
          added.received += 1
          lane.push(groupId, object, cached)
        },
        listener.onEnded
      )
    } catch (error) {
      lane.close()
      throw error
    }
    return added
  }

  /// The sound approaches `volume` with this time constant, as in
  /// `AudioParam.setTargetAtTime`.
  setVolume(volume: number, timeConstantSeconds?: number): void {
    this.lane.fadeTo(volume, timeConstantSeconds)
  }

  /// The objects received, the ones dropped as late included.
  get receivedObjects(): number {
    return this.received
  }

  /// The capture timestamp heard now; undefined until a chunk has played.
  get captureMicros(): number | undefined {
    return this.lane.positionMicros()
  }

  /// Chunks that arrived after they were due, not counting the relay's cache
  /// of the group the subscription joined.
  get droppedLate(): number {
    return this.lane.droppedLate
  }

  /// Unsubscribes and releases the decoder. A second call does nothing.
  async close(): Promise<void> {
    this.onClosed()
    this.lane.close()
    await this.unsubscribe()
  }
}
