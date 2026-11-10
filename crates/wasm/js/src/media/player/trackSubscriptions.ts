import type { MoqSubscription, PublishDone } from '../../core/session'
import {
  type MediaKind,
  type ObjectHandler,
  type PlayedTrack,
  type TrackContext,
  observedObjectHandler,
  readGroups,
  readJoinedObjects,
  sessionOf,
  streamId
} from './trackContext'
import { getErrorMessage } from '../internal/status'
import { type MediaCatalogTrack, catalogTrackNamespace } from '../catalog'

/// draft-14 §7.2: the relay sends the lower subscriber priority value first,
/// so audio takes the downlink ahead of video.
const SUBSCRIBER_PRIORITY: Record<MediaKind, number> = { video: 128, audio: 0 }

/// A group whose largest object ID is below this is read from its start
/// through a joining FETCH; a longer one is not worth catching up on when the
/// publisher starts a new group on request. 2 seconds of video at 30 fps,
/// the time within which the relay passes on only one such request.
const FETCHED_GROUP_OBJECTS = 60

export type TrackSubscription = PlayedTrack & {
  id: bigint
  handle: MoqSubscription
  /// Set once the player let go of it, so objects still in flight are dropped.
  forgotten: boolean
}

/// Review plays what FETCH brings, so the live subscriptions stop forwarding
/// for its duration and deliver again, from the next group, on the way back.
export class TrackSubscriptions {
  private readonly subscriptions = new Map<MediaKind, TrackSubscription>()
  private readonly retiring = new Set<TrackSubscription>()
  private forwardPaused = false
  private forwardUpdate = Promise.resolve()

  constructor(
    private readonly context: TrackContext,
    /// `leading` when no video is subscribed after the PUBLISH_DONE: the
    /// video's, or the audio's of a broadcast played without video.
    private readonly onPublishDone: (kind: MediaKind, done: PublishDone, leading: boolean) => void
  ) {}

  get(kind: MediaKind): TrackSubscription | undefined {
    return this.subscriptions.get(kind)
  }

  entries(): [MediaKind, TrackSubscription][] {
    return [...this.subscriptions]
  }

  get paused(): boolean {
    return this.forwardPaused
  }

  /// `groupStart` starts the subscription at the first object of a group
  /// (draft-18 §5.1.3.1): the group the largest object is in, through a
  /// joining FETCH, or a new group the publisher is asked to start when that
  /// group's largest object ID is `FETCHED_GROUP_OBJECTS` or more. While forwarding is paused
  /// the subscription starts with the next group instead.
  async subscribe(
    kind: MediaKind,
    wireName: string,
    track: MediaCatalogTrack,
    handler: ObjectHandler,
    groupStart = false
  ): Promise<void> {
    this.subscriptions.set(kind, await this.open(kind, wireName, track, handler, groupStart))
  }

  /// Subscribes without making the subscription the one of its kind, so a
  /// rendition switch can receive the next rendition while the current one
  /// still plays; `handOver` gives it the kind.
  async open(
    kind: MediaKind,
    wireName: string,
    track: MediaCatalogTrack,
    handler: ObjectHandler,
    groupStart = false
  ): Promise<TrackSubscription> {
    const namespace = catalogTrackNamespace(track, this.context.namespace)
    const session = sessionOf(this.context)
    const joining = groupStart && !this.forwardPaused
    const handle = joining
      ? await session.subscribeGroupStart(namespace, wireName, FETCHED_GROUP_OBJECTS)
      : await session.subscribe(namespace, wireName, {
          forward: !this.forwardPaused,
          priority: SUBSCRIBER_PRIORITY[kind]
        })
    const subscription: TrackSubscription = { id: streamId(), handle, name: wireName, track, forgotten: false }
    const observed = observedObjectHandler(this.context, subscription.id, wireName, handler)
    const read = joining ? readJoinedObjects : readGroups
    void read(this.context, subscription.id, handle, (groupId, object, cached) => {
      if (!subscription.forgotten) {
        observed(groupId, object, cached)
      }
    })
    void handle.done.then((done) => done && this.end(kind, subscription, done))
    this.context.log('info', `subscribed ${namespace.join('/')}/${wireName}`)
    return subscription
  }

  /// Asks the publisher of `subscription` to start a new group, for a reader
  /// that can only start at one. Never rejects: a track that does not start
  /// groups on request, or a request that fails, leaves the reader waiting
  /// for the track's next group.
  async requestNewGroup(subscription: TrackSubscription, largestGroup?: bigint): Promise<void> {
    try {
      await subscription.handle.requestNewGroup(largestGroup)
    } catch (error) {
      this.context.log('warn', `new group of ${subscription.name}: ${getErrorMessage(error)}`)
    }
  }

  /// Makes `next` the subscription of its kind. The one it replaces keeps
  /// delivering until `retire` unsubscribes it or its publisher ends it.
  handOver(kind: MediaKind, next: TrackSubscription): TrackSubscription | undefined {
    const previous = this.subscriptions.get(kind)
    this.subscriptions.set(kind, next)
    if (previous) {
      this.retiring.add(previous)
    }
    return previous
  }

  async retire(subscription: TrackSubscription): Promise<void> {
    if (this.retiring.delete(subscription)) {
      await this.close(subscription)
    }
  }

  /// Lets go of every subscription, retiring ones included, before it awaits
  /// anything. Forwarding is no longer paused, and no SUBSCRIBE_UPDATE is sent
  /// to resume it.
  async unsubscribeAll(): Promise<void> {
    const subscriptions = [...this.subscriptions.values(), ...this.retiring]
    this.subscriptions.clear()
    this.retiring.clear()
    this.forwardPaused = false
    for (const subscription of subscriptions) {
      await this.close(subscription)
    }
  }

  async unsubscribe(kind: MediaKind): Promise<void> {
    const subscription = this.subscriptions.get(kind)
    if (!subscription) {
      return
    }
    this.subscriptions.delete(kind)
    await this.close(subscription)
  }

  /// Never rejects: a failed UNSUBSCRIBE is logged, so a teardown goes on to
  /// the other tracks.
  async close(subscription: TrackSubscription): Promise<void> {
    this.forget(subscription)
    if (sessionOf(this.context).isOpen) {
      try {
        await subscription.handle.close()
      } catch (error) {
        this.context.log('warn', `unsubscribe ${subscription.name}: ${getErrorMessage(error)}`)
        return
      }
    }
    this.context.log('info', `unsubscribed ${subscription.name}`)
  }

  /// Delivers the groups before `groupId` and lets the publisher end the
  /// subscription with PUBLISH_DONE once they are sent.
  async endBeforeGroup(subscription: TrackSubscription, groupId: bigint): Promise<void> {
    if (sessionOf(this.context).isOpen) {
      await subscription.handle.endBeforeGroup(groupId, !this.forwardPaused)
    }
  }

  private end(kind: MediaKind, subscription: TrackSubscription, done: PublishDone): void {
    if (this.retiring.delete(subscription)) {
      this.forget(subscription)
      this.context.log('info', `${subscription.name} ended by the publisher (status ${done.code})`)
      return
    }
    if (this.subscriptions.get(kind) !== subscription) {
      return
    }
    this.subscriptions.delete(kind)
    this.context.observer.forget(subscription.id)
    this.context.log('info', `${subscription.name} ended by the publisher (status ${done.code})`)
    this.onPublishDone(kind, done, !this.subscriptions.has('video'))
  }

  private forget(subscription: TrackSubscription): void {
    subscription.forgotten = true
    this.context.observer.forget(subscription.id)
  }

  pauseForward(): void {
    if (this.forwardPaused) {
      return
    }
    this.forwardPaused = true
    this.updateForward(false)
  }

  resumeForward(): boolean {
    if (!this.forwardPaused) {
      return false
    }
    this.forwardPaused = false
    this.updateForward(true)
    return true
  }

  /// A pause and a resume in quick succession must reach every subscription in
  /// that order, so the updates are chained rather than sent concurrently.
  private updateForward(forward: boolean): void {
    this.forwardUpdate = this.forwardUpdate.then(() => this.setForward(forward))
  }

  private async setForward(forward: boolean): Promise<void> {
    if (!this.context.session?.isOpen) {
      return
    }
    for (const [kind, subscription] of this.subscriptions) {
      try {
        await subscription.handle.setForward(forward)
      } catch (error) {
        this.context.log('error', `forward ${subscription.name}: ${getErrorMessage(error)}`)
        continue
      }
      // Delivery resumes with the next group, which the video would
      // otherwise wait a keyframe interval for.
      if (forward && kind === 'video') {
        await this.requestNewGroup(subscription)
      }
    }
    this.context.log('info', `live subscriptions ${forward ? 'resumed' : 'paused'}`)
  }
}
