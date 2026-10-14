import { type MediaKind, type TrackContext, observedObjectHandler } from './trackContext'
import type { SubgroupObjectHandler } from '@moqt/subscriptionStateManager'
import type { PublishDoneMessage } from '../../pkg/moqt'
import { getErrorMessage } from '../../utils/page'
import type { MediaCatalogTrack } from '../msf/catalog'

export type TrackSubscription = {
  requestId: bigint
  trackAlias: bigint
  name: string
  track: MediaCatalogTrack
}

/// Review plays what FETCH brings, so the live subscriptions stop forwarding
/// for its duration and deliver again, from the next group, on the way back.
export class TrackSubscriptions {
  private readonly subscriptions = new Map<MediaKind, TrackSubscription>()
  private readonly retiring = new Map<bigint, TrackSubscription>()
  private forwardPaused = false
  private forwardUpdate = Promise.resolve()

  constructor(
    private readonly context: TrackContext,
    private readonly onPublishDone: (kind: MediaKind, message: PublishDoneMessage) => void
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

  async subscribe(
    kind: MediaKind,
    wireName: string,
    track: MediaCatalogTrack,
    handler: SubgroupObjectHandler
  ): Promise<void> {
    this.subscriptions.set(kind, await this.open(kind, wireName, track, handler))
  }

  /// Subscribes without making the subscription the one of its kind, so a
  /// rendition switch can receive the next rendition while the current one
  /// still plays; `handOver` gives it the kind.
  async open(
    kind: MediaKind,
    wireName: string,
    track: MediaCatalogTrack,
    handler: SubgroupObjectHandler
  ): Promise<TrackSubscription> {
    const { client, namespace, authInfo } = this.context
    const { requestId, subscribeOk } = await client.subscribe(namespace, wireName, authInfo, {
      forward: true,
      onPublishDone: (message) => this.end(kind, message)
    })
    const subscription = { requestId, trackAlias: subscribeOk.trackAlias, name: wireName, track }
    if (this.forwardPaused) {
      await client.setSubscriptionForward(requestId, false)
    }
    client.setOnSubgroupObjectHandler(
      subscribeOk.trackAlias,
      observedObjectHandler(this.context, subscribeOk.trackAlias, wireName, handler)
    )
    this.context.log('info', `subscribed ${namespace.join('/')}/${wireName}`)
    return subscription
  }

  /// Makes `next` the subscription of its kind. The one it replaces keeps
  /// delivering until `retire` unsubscribes it or its publisher ends it.
  handOver(kind: MediaKind, next: TrackSubscription): TrackSubscription | undefined {
    const previous = this.subscriptions.get(kind)
    this.subscriptions.set(kind, next)
    if (previous) {
      this.retiring.set(previous.requestId, previous)
    }
    return previous
  }

  async retire(subscription: TrackSubscription): Promise<void> {
    if (this.retiring.delete(subscription.requestId)) {
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
    const { client } = this.context
    this.forget(subscription)
    if (client.getConnectionStatus()) {
      await client.unsubscribe(subscription.requestId)
    }
    this.context.log('info', `unsubscribed ${subscription.name}`)
  }

  /// Delivers the groups before `groupId` and lets the publisher end the
  /// subscription with PUBLISH_DONE once they are sent.
  async endBeforeGroup(subscription: TrackSubscription, groupId: bigint): Promise<void> {
    const { client } = this.context
    if (client.getConnectionStatus()) {
      await client.endSubscriptionBeforeGroup(subscription.requestId, groupId, !this.forwardPaused)
    }
  }

  private end(kind: MediaKind, message: PublishDoneMessage): void {
    const retired = this.retiring.get(message.requestId)
    if (retired) {
      this.retiring.delete(message.requestId)
      this.forget(retired)
      this.context.log('info', `${retired.name} ended by the publisher (status ${message.statusCode})`)
      return
    }
    const subscription = this.subscriptions.get(kind)
    if (subscription?.requestId !== message.requestId) {
      return
    }
    this.subscriptions.delete(kind)
    this.context.observer.forget(subscription.trackAlias)
    this.context.log('info', `${subscription.name} ended by the publisher (status ${message.statusCode})`)
    this.onPublishDone(kind, message)
  }

  private forget(subscription: TrackSubscription): void {
    this.context.client.clearSubgroupObjectHandler(subscription.trackAlias)
    this.context.observer.forget(subscription.trackAlias)
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
    const { client } = this.context
    if (!client.getConnectionStatus()) {
      return
    }
    for (const subscription of this.subscriptions.values()) {
      try {
        await client.setSubscriptionForward(subscription.requestId, forward)
      } catch (error) {
        this.context.log('error', `forward ${subscription.name}: ${getErrorMessage(error)}`)
      }
    }
    this.context.log('info', `live subscriptions ${forward ? 'resumed' : 'paused'}`)
  }
}
