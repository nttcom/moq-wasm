import type { ReceivedObject } from '../../core/session'
import type { MediaCatalogTrack } from '../catalog'
import { getErrorMessage } from '../internal/status'
import { RenditionSplice, type SpliceStart } from './renditionSplice'
import type { LogLevel } from './trackContext'
import type { TrackSubscription, TrackSubscriptions } from './trackSubscriptions'

export type VideoSwitchSubscriptions = Pick<
  TrackSubscriptions,
  'get' | 'open' | 'close' | 'handOver' | 'requestNewGroup' | 'endBeforeGroup' | 'retire'
>

export type VideoSwitchHost = {
  subscriptions: VideoSwitchSubscriptions
  play(trackName: string, groupId: bigint, object: ReceivedObject): void
  handedOver(next: TrackSubscription, start: SpliceStart): void
  log(level: LogLevel, message: string): void
}

/// A live LOC video rendition change in progress: `next` is subscribed next
/// to `outgoing` until the splice has handed playback over to it.
type VideoSwitch = {
  splice: RenditionSplice
  outgoing: TrackSubscription
  next?: TrackSubscription
  previousName: string
}

/// Switches live LOC video to another rendition of its switching set
/// without a gap: the rendition subscribed so far plays on until the new one
/// takes over at a group boundary (see `RenditionSplice`).
export class VideoSwitcher {
  private current: VideoSwitch | undefined

  constructor(private readonly host: VideoSwitchHost) {}

  /// A switch waits for its incoming rendition to play.
  get waiting(): boolean {
    return this.current !== undefined && !this.current.splice.incomingPlaying
  }

  /// Subscribes `name` next to `outgoing`. A switch that fails to subscribe
  /// stays in progress, for the caller to `abandon`.
  async begin(
    outgoing: TrackSubscription,
    name: string,
    track: MediaCatalogTrack,
    outgoingNewestGroup: bigint | undefined,
    previousName: string
  ): Promise<void> {
    const pending: VideoSwitch = { splice: new RenditionSplice(outgoingNewestGroup), outgoing, previousName }
    this.current = pending
    const subscriptions = this.host.subscriptions
    const next = await subscriptions.open('video', name, track, (groupId, object) =>
      this.handleIncoming(pending, name, groupId, object)
    )
    if (this.current !== pending) {
      await subscriptions.close(next)
      return
    }
    pending.next = next
    // The splice waits for a group start: renditions keyframe on the same
    // frames, so the new group begins on the outgoing rendition too.
    void subscriptions.requestNewGroup(next, next.handle.largest?.groupId)
  }

  /// False for an object of the outgoing rendition at or past the start of
  /// the incoming one.
  acceptOutgoing(trackName: string, groupId: bigint): boolean {
    const pending = this.current
    return pending?.outgoing.name !== trackName || pending.splice.acceptOutgoing(groupId) === 'play'
  }

  /// Settles a switch in progress: one that has handed over keeps the new
  /// rendition and drops the outgoing one, one that has not keeps playing the
  /// outgoing rendition and returns the name selected before it.
  abandon(): string | undefined {
    const pending = this.current
    if (!pending) {
      return undefined
    }
    if (pending.splice.start !== undefined) {
      this.retireOutgoing(pending)
      return undefined
    }
    this.current = undefined
    if (pending.next) {
      void this.host.subscriptions.close(pending.next)
    }
    return pending.previousName
  }

  /// Objects that arrive before the subscription is known to the switch are
  /// dropped, so the splice is decided by a group start seen afterwards.
  private handleIncoming(pending: VideoSwitch, trackName: string, groupId: bigint, object: ReceivedObject): void {
    const next = pending.next
    if (!next) {
      return
    }
    if (this.current !== pending) {
      // A switch settled after the handover, by a seek or another track
      // change, can still receive the rest of the group before the start.
      if (
        this.host.subscriptions.get('video') === next &&
        pending.splice.acceptIncoming(groupId, object.objectId).play
      ) {
        this.host.play(trackName, groupId, object)
      }
      return
    }
    const verdict = pending.splice.acceptIncoming(groupId, object.objectId)
    if (verdict.start) {
      this.startIncoming(pending, next, verdict.start)
      if (!verdict.play) {
        // The outgoing rendition delivered this group first, so the switch
        // starts at the next one, which a long group interval would delay.
        void this.host.subscriptions.requestNewGroup(next, groupId)
      }
    }
    if (verdict.retireOutgoing) {
      this.retireOutgoing(pending)
    }
    if (verdict.play) {
      this.host.play(trackName, groupId, object)
    }
  }

  private startIncoming(pending: VideoSwitch, next: TrackSubscription, start: SpliceStart): void {
    this.host.subscriptions.handOver('video', next)
    this.host.handedOver(next, start)
    if (!start.aligned) {
      return
    }
    this.host.subscriptions.endBeforeGroup(pending.outgoing, start.groupId).catch((error) => {
      this.host.log('warn', `end ${pending.outgoing.name} before group ${start.groupId}: ${getErrorMessage(error)}`)
      this.retireOutgoing(pending)
    })
  }

  private retireOutgoing(pending: VideoSwitch): void {
    if (this.current !== pending) {
      return
    }
    this.current = undefined
    void this.host.subscriptions.retire(pending.outgoing)
  }
}
