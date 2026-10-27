import type { MoqSubscription, PublishDone } from '@moqt/session'
import {
  type MediaKind,
  type ObjectHandler,
  type TrackContext,
  observedObjectHandler,
  readGroups,
  sessionOf,
  streamId
} from './trackContext'
import { getErrorMessage } from '../../utils/page'
import type { MediaCatalogTrack } from '../msf/catalog'

export type TrackSubscription = {
  /// Keys the subscription in the delivery observer.
  id: bigint
  handle: MoqSubscription
  name: string
  track: MediaCatalogTrack
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
    private readonly onPublishDone: (kind: MediaKind, done: PublishDone) => void
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

  async subscribe(kind: MediaKind, wireName: string, track: MediaCatalogTrack, handler: ObjectHandler): Promise<void> {
    this.subscriptions.set(kind, await this.open(kind, wireName, track, handler))
  }

  /// Subscribes without making the subscription the one of its kind, so a
  /// rendition switch can receive the next rendition while the current one
  /// still plays; `handOver` gives it the kind.
  async open(
    kind: MediaKind,
    wireName: string,
    track: MediaCatalogTrack,
    handler: ObjectHandler
  ): Promise<TrackSubscription> {
    const { namespace } = this.context
    const handle = await sessionOf(this.context).subscribe(namespace, wireName, { forward: !this.forwardPaused })
    const subscription: TrackSubscription = { id: streamId(), handle, name: wireName, track, forgotten: false }
    const observed = observedObjectHandler(this.context, subscription.id, wireName, handler)
    void readGroups(this.context, subscription.id, handle, (groupId, object) => {
      if (!subscription.forgotten) {
        observed(groupId, object)
      }
    })
    void handle.done.then((done) => done && this.end(kind, subscription, done))
    this.context.log('info', `subscribed ${namespace.join('/')}/${wireName}`)
    return subscription
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

  async retireAll(): Promise<void> {
    for (const subscription of [...this.retiring.values()]) {
      await this.retire(subscription)
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

  async close(subscription: TrackSubscription): Promise<void> {
    this.forget(subscription)
    if (sessionOf(this.context).isOpen) {
      await subscription.handle.close()
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
    this.onPublishDone(kind, done)
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
    for (const subscription of this.subscriptions.values()) {
      try {
        await subscription.handle.setForward(forward)
      } catch (error) {
        this.context.log('error', `forward ${subscription.name}: ${getErrorMessage(error)}`)
      }
    }
    this.context.log('info', `live subscriptions ${forward ? 'resumed' : 'paused'}`)
  }
}
