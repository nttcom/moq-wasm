import type { MoqSubscription, PublishDone } from '../../core/session'
import { type TrackContext, observedObjectHandler, sessionOf, streamId } from './trackContext'

export type TextTrackHandler = (text: string, groupId: bigint) => void

export type TextTrackOptions = {
  /// Reads the group the largest object is in from its first object too
  /// (SUBSCRIBE with a joining FETCH, draft-ietf-moq-msf-01 §5).
  currentGroup?: boolean
  /// Called when the publisher ends the track with PUBLISH_DONE.
  onEnded?: (done: PublishDone) => void
}

export class TextTracks {
  private readonly subscribed: { id: bigint; handle: MoqSubscription }[] = []

  constructor(private readonly context: TrackContext) {}

  async subscribe(name: string, onText: TextTrackHandler, options: TextTrackOptions = {}): Promise<void> {
    const { namespace } = this.context
    const session = sessionOf(this.context)
    const handle = options.currentGroup
      ? await session.subscribeCurrentGroup(namespace, name)
      : await session.subscribe(namespace, name)
    const subscription = { id: streamId(), handle }
    this.subscribed.push(subscription)
    const observed = observedObjectHandler(this.context, subscription.id, name, (groupId, object) =>
      deliverText(object.objectPayload, groupId, onText)
    )
    void handle.done.then((done) => done && this.end(name, subscription, done, options.onEnded))
    void (async () => {
      for await (const object of handle) {
        if (this.subscribed.includes(subscription)) {
          observed(object.groupId, object)
        }
      }
    })().catch((error: unknown) => this.context.log('info', `${name} read ended: ${error}`))
    this.context.log('info', `subscribed ${namespace.join('/')}/${name}`)
  }

  private end(
    name: string,
    subscription: { id: bigint; handle: MoqSubscription },
    done: PublishDone,
    onEnded: ((done: PublishDone) => void) | undefined
  ): void {
    const index = this.subscribed.indexOf(subscription)
    if (index < 0) {
      return
    }
    this.subscribed.splice(index, 1)
    this.context.log('info', `${name} ended by the publisher`)
    onEnded?.(done)
  }

  async unsubscribeAll(): Promise<void> {
    for (const { id, handle } of this.subscribed.splice(0)) {
      this.context.observer.forget(id)
      if (this.context.session?.isOpen) {
        await handle.close()
      }
    }
  }
}

function deliverText(payload: Uint8Array, groupId: bigint, onText: TextTrackHandler): void {
  if (payload.byteLength > 0) {
    onText(new TextDecoder().decode(payload), groupId)
  }
}
