import type { MoqSubscription, PublishDone } from '../../../core/session'
import type { MediaCatalogTrack } from '../../catalog'
import type { MediaKind, ObjectHandler } from '../trackContext'
import type { TrackSubscription } from '../trackSubscriptions'
import type { VideoSwitchSubscriptions } from '../videoSwitcher'
import { receivedObject } from './fakeSession'

/// Records each call as `open camera`, `handOver camera`, `close camera`,
/// `retire camera_low`, `endBeforeGroup camera_low 5` or
/// `requestNewGroup camera 4`; `deliver` hands an object to the handler of
/// the subscription `open` made for a track.
export class FakeTrackSubscriptions implements VideoSwitchSubscriptions {
  readonly calls: string[] = []
  private readonly subscriptions = new Map<MediaKind, TrackSubscription>()
  private readonly handlers = new Map<string, ObjectHandler>()
  private readonly publishDones = new Map<string, (done: PublishDone) => void>()
  private readonly opened = new Map<string, TrackSubscription>()
  /// Makes `endBeforeGroup` reject, as when the SUBSCRIBE_UPDATE cannot be sent.
  endBeforeGroupError: Error | undefined
  /// The largest group SUBSCRIBE_OK reports for the subscriptions `open` makes.
  openedLargestGroup: bigint | undefined

  /// Makes a subscription to `name` the one of `kind`, as `subscribe` does.
  subscribed(kind: MediaKind, name: string): TrackSubscription {
    const subscription = this.trackSubscription(name, { name, label: name })
    this.subscriptions.set(kind, subscription)
    return subscription
  }

  deliver(name: string, groupId: bigint, objectId: bigint): void {
    this.handlers.get(name)?.(groupId, receivedObject(groupId, objectId), false)
  }

  /// Resolves `done` of the subscription to `name` with SUBSCRIPTION_ENDED,
  /// and waits for the callbacks chained on it.
  async endedByPublisher(name: string): Promise<void> {
    this.publishDones.get(name)?.({ code: 'SUBSCRIPTION_ENDED', reason: '' })
    await Promise.resolve()
  }

  get(kind: MediaKind): TrackSubscription | undefined {
    return this.subscriptions.get(kind)
  }

  async open(
    _kind: MediaKind,
    wireName: string,
    track: MediaCatalogTrack,
    handler: ObjectHandler
  ): Promise<TrackSubscription> {
    this.calls.push(`open ${wireName}`)
    this.handlers.set(wireName, handler)
    const subscription = this.trackSubscription(wireName, track, this.openedLargestGroup)
    this.opened.set(wireName, subscription)
    return subscription
  }

  /// The subscription `open` made for `name`.
  openedFor(name: string): TrackSubscription | undefined {
    return this.opened.get(name)
  }

  async close(subscription: TrackSubscription): Promise<void> {
    this.calls.push(`close ${subscription.name}`)
  }

  handOver(kind: MediaKind, next: TrackSubscription): TrackSubscription | undefined {
    this.calls.push(`handOver ${next.name}`)
    const previous = this.subscriptions.get(kind)
    this.subscriptions.set(kind, next)
    return previous
  }

  async requestNewGroup(subscription: TrackSubscription, largestGroup?: bigint): Promise<void> {
    this.calls.push(`requestNewGroup ${subscription.name} ${largestGroup}`)
  }

  async endBeforeGroup(subscription: TrackSubscription, groupId: bigint): Promise<void> {
    this.calls.push(`endBeforeGroup ${subscription.name} ${groupId}`)
    if (this.endBeforeGroupError) {
      throw this.endBeforeGroupError
    }
  }

  async retire(subscription: TrackSubscription): Promise<void> {
    this.calls.push(`retire ${subscription.name}`)
  }

  private trackSubscription(name: string, track: MediaCatalogTrack, largestGroup?: bigint): TrackSubscription {
    const done = new Promise<PublishDone>((resolve) => this.publishDones.set(name, resolve))
    const largest = largestGroup === undefined ? undefined : { groupId: largestGroup, objectId: 0n }
    return {
      id: 0n,
      handle: { largest, done } as unknown as MoqSubscription,
      priority: 128,
      forward: true,
      name,
      track,
      forgotten: false
    }
  }
}
