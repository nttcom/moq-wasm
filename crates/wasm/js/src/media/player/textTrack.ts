import type { MoqSubscription, PublishDone } from '../../core/session'
import { type TrackContext, observedObjectHandler, sessionOf, streamId } from './trackContext'
import { getErrorMessage } from '../internal/status'

export type TextTrackHandler = (text: string, groupId: bigint) => void

export type TextTrackOptions = {
  /// Reads the group the largest object is in from its first object too
  /// (SUBSCRIBE with a joining FETCH, draft-ietf-moq-msf-01 §5).
  currentGroup?: boolean
  /// Called when the publisher ends the track with PUBLISH_DONE.
  onEnded?: (done: PublishDone) => void
}

type TextSubscription = { id: bigint; name: string; handle: MoqSubscription }

export class TextTracks {
  private readonly subscribed: TextSubscription[] = []

  constructor(private readonly context: TrackContext) {}

  async subscribe(name: string, onText: TextTrackHandler, options: TextTrackOptions = {}): Promise<void> {
    const { namespace } = this.context
    const session = sessionOf(this.context)
    const handle = options.currentGroup
      ? await session.subscribeCurrentGroup(namespace, name)
      : await session.subscribe(namespace, name)
    const subscription = { id: streamId(), name, handle }
    this.subscribed.push(subscription)
    const observed = observedObjectHandler(this.context, subscription.id, name, (groupId, object) =>
      deliverText(object.objectPayload, groupId, onText)
    )
    void handle.done.then((done) => done && this.end(name, subscription, done, options.onEnded))
    void (async () => {
      for await (const object of handle) {
        if (this.subscribed.includes(subscription)) {
          observed(object.groupId, object, false)
        }
      }
    })().catch((error: unknown) => this.context.log('info', `${name} read ended: ${error}`))
    this.context.log('info', `subscribed ${namespace.join('/')}/${name}`)
  }

  private end(
    name: string,
    subscription: TextSubscription,
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

  /// Never rejects: a failed UNSUBSCRIBE is logged and the other tracks are
  /// still unsubscribed.
  async unsubscribeAll(): Promise<void> {
    for (const { id, name, handle } of this.subscribed.splice(0)) {
      this.context.observer.forget(id)
      if (this.context.session?.isOpen) {
        await handle.close().catch((error: unknown) => {
          this.context.log('warn', `unsubscribe ${name}: ${getErrorMessage(error)}`)
        })
      }
    }
  }
}

function deliverText(payload: Uint8Array, groupId: bigint, onText: TextTrackHandler): void {
  if (payload.byteLength > 0) {
    onText(new TextDecoder().decode(payload), groupId)
  }
}
