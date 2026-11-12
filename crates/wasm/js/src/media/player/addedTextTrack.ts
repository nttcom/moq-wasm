import type { PublishDone } from '../../core/session'
import { readLocHeader } from '../internal/loc'
import { subscribeAddedTrack } from './addedTrack'
import type { LivePlayout } from './livePlayout'
import type { TrackContext } from './trackContext'

export type AddedTextTrackListener = {
  /// The payload of an object, as UTF-8 text, when the sound captured with it
  /// plays, in the order received. `lateMs` is how long after that time it is
  /// delivered: 0 unless the object arrived late. An object without a capture
  /// timestamp, or one that arrives before the player's sound has started,
  /// has no time to wait for and is delivered at once with `lateMs` 0.
  onText(text: string, lateMs: number): void
  /// The publisher ended the track with PUBLISH_DONE.
  onEnded?(done: PublishDone): void
}

/// A text track `LivePlayer.addTextTrack` delivers on the live clock, such
/// as the captions of the sound.
export class AddedTextTrack {
  private readonly timers = new Set<ReturnType<typeof setTimeout>>()
  private lastDeliveryMs = 0
  private unsubscribe: () => Promise<void> = async () => {}

  private constructor(private readonly onClosed: () => void) {}

  static async open(
    context: TrackContext,
    playout: Pick<LivePlayout, 'dueAt'>,
    name: string,
    listener: AddedTextTrackListener,
    onClosed: () => void
  ): Promise<AddedTextTrack> {
    const added = new AddedTextTrack(onClosed)
    const decoder = new TextDecoder()
    added.unsubscribe = await subscribeAddedTrack(
      context,
      context.namespace,
      name,
      (_groupId, object) => {
        const captureMicros = readLocHeader(object.locHeader).timestampMicros
        const dueMs = captureMicros === undefined ? undefined : playout.dueAt(captureMicros)
        const nowMs = performance.now()
        const lateMs = dueMs === undefined ? 0 : Math.max(0, nowMs - dueMs)
        // Timer delays are whole milliseconds, so two texts due together could fire out of order.
        added.lastDeliveryMs = Math.max(dueMs ?? nowMs, nowMs, added.lastDeliveryMs + 1)
        const text = decoder.decode(object.objectPayload)
        const timer = setTimeout(() => {
          added.timers.delete(timer)
          listener.onText(text, lateMs)
        }, added.lastDeliveryMs - nowMs)
        added.timers.add(timer)
      },
      listener.onEnded
    )
    return added
  }

  /// Unsubscribes and drops the text still waiting for its time.
  async close(): Promise<void> {
    this.onClosed()
    for (const timer of this.timers) {
      clearTimeout(timer)
    }
    this.timers.clear()
    await this.unsubscribe()
  }
}
