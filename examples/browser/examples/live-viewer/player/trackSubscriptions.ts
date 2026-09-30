import { getErrorMessage } from '../../media/common'
import type { MediaCatalogTrack } from '../../media/catalog'
import { type MediaKind, type TrackContext, observedObjectHandler } from './trackContext'
import type { SubgroupObjectHandler } from '@moqt/subscriptionStateManager'

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
  private forwardPaused = false
  private forwardUpdate = Promise.resolve()

  constructor(private readonly context: TrackContext) {}

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
    const { client, namespace, authInfo } = this.context
    const { requestId, subscribeOk } = await client.subscribe(namespace, wireName, authInfo, { forward: true })
    this.subscriptions.set(kind, { requestId, trackAlias: subscribeOk.trackAlias, name: wireName, track })
    if (this.forwardPaused) {
      await client.setSubscriptionForward(requestId, false)
    }
    client.setOnSubgroupObjectHandler(
      subscribeOk.trackAlias,
      observedObjectHandler(this.context, subscribeOk.trackAlias, wireName, handler)
    )
    this.context.log('info', `subscribed ${namespace.join('/')}/${wireName}`)
  }

  async unsubscribe(kind: MediaKind): Promise<void> {
    const subscription = this.subscriptions.get(kind)
    if (!subscription) {
      return
    }

    const { client, observer } = this.context
    this.subscriptions.delete(kind)
    client.clearSubgroupObjectHandler(subscription.trackAlias)
    observer.forget(subscription.trackAlias)
    if (client.getConnectionStatus()) {
      await client.unsubscribe(subscription.requestId)
    }
    this.context.log('info', `unsubscribed ${subscription.name}`)
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
