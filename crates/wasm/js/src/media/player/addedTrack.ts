import type { PublishDone } from '../../core/session'
import { getErrorMessage } from '../internal/status'
import { type ObjectHandler, type TrackContext, readJoinedObjects, sessionOf, streamId } from './trackContext'

/// Subscribes a track a page added to a player, with a joining FETCH so the
/// group under way is read from its start. Returns the function that
/// unsubscribes: it never rejects (a failed UNSUBSCRIBE is logged), and
/// calling it again does nothing. Rejects when the SUBSCRIBE does.
export async function subscribeAddedTrack(
  context: TrackContext,
  namespace: string[],
  name: string,
  onObject: ObjectHandler,
  onEnded: ((done: PublishDone) => void) | undefined
): Promise<() => Promise<void>> {
  const subscription = await sessionOf(context).subscribeCurrentGroup(namespace, name)
  let closed = false
  void readJoinedObjects(context, streamId(), subscription, (groupId, object, cached) => {
    if (!closed) {
      onObject(groupId, object, cached)
    }
  })
  void subscription.done.then((done) => {
    if (done && !closed) {
      onEnded?.(done)
    }
  })
  return async () => {
    if (closed) {
      return
    }
    closed = true
    if (context.session?.isOpen) {
      await subscription.close().catch((error: unknown) => {
        context.log('warn', `unsubscribe ${name}: ${getErrorMessage(error)}`)
      })
    }
  }
}
