import type { MoqSubscription } from '../../../core/session'
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

  /// Makes a subscription to `name` the one of `kind`, as `subscribe` does.
  subscribed(kind: MediaKind, name: string): TrackSubscription {
    const subscription = trackSubscription(name, { name, label: name })
    this.subscriptions.set(kind, subscription)
    return subscription
  }

  deliver(name: string, groupId: bigint, objectId: bigint): void {
    this.handlers.get(name)?.(groupId, receivedObject(groupId, objectId), false)
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
    return trackSubscription(wireName, track)
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
  }

  async retire(subscription: TrackSubscription): Promise<void> {
    this.calls.push(`retire ${subscription.name}`)
  }
}

function trackSubscription(name: string, track: MediaCatalogTrack): TrackSubscription {
  return {
    id: 0n,
    handle: { largest: undefined } as unknown as MoqSubscription,
    priority: 128,
    name,
    track,
    forgotten: false
  }
}
