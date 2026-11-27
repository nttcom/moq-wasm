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

type VideoSwitchHost = {
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
  /// The largest group of `next` when its new group was asked for.
  requestedAfter?: bigint
  /// Whether the new group was asked for again after it was reset.
  askedAgain: boolean
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
    const pending: VideoSwitch = {
      splice: new RenditionSplice(outgoingNewestGroup),
      outgoing,
      previousName,
      askedAgain: false
    }
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
    pending.requestedAfter = next.handle.largest?.groupId
    // The splice waits for a group start: renditions keyframe on the same
    // frames, so the new group begins on the outgoing rendition too.
    void subscriptions.requestNewGroup(next, pending.requestedAfter)
  }

  /// A group of the incoming rendition that `begin` asked for and that was
  /// reset before the splice started leaves the switch waiting for the next
  /// keyframe, so the group is asked for once more.
  groupReset(subscription: TrackSubscription, groupId: bigint): void {
    const pending = this.current
    if (
      !pending ||
      pending.next !== subscription ||
      pending.splice.start !== undefined ||
      pending.askedAgain ||
      (pending.requestedAfter !== undefined && groupId <= pending.requestedAfter)
    ) {
      return
    }
    pending.askedAgain = true
    this.host.log('info', `group ${groupId} of ${subscription.name} reset before the switch, asking for a new group`)
    void this.host.subscriptions.requestNewGroup(subscription, groupId)
  }

  /// False for an object of the outgoing rendition at or past the start of
  /// the incoming one. An object past the start group also retires the
  /// outgoing subscription (`overrun`).
  acceptOutgoing(trackName: string, groupId: bigint): boolean {
    const pending = this.current
    if (pending?.outgoing.name !== trackName) {
      return true
    }
    const verdict = pending.splice.acceptOutgoing(groupId)
    if (verdict === 'overrun') {
      this.retireOutgoing(pending)
    }
    return verdict === 'play'
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
    if (verdict.play) {
      this.host.play(trackName, groupId, object)
    }
  }

  private startIncoming(pending: VideoSwitch, next: TrackSubscription, start: SpliceStart): void {
    this.host.subscriptions.handOver('video', next)
    this.host.handedOver(next, start)
    if (!start.aligned) {
      this.retireOutgoing(pending)
      return
    }
    // The outgoing rendition can still be delivering the group before the
    // start, so the switch waits for the publisher to end it there.
    void pending.outgoing.handle.done.then(() => {
      if (this.current === pending) {
        this.current = undefined
      }
    })
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
