import type { MoqSubscription, ReceivedObject } from 'moq-wasm'

/// The objects of a track that writes one object per group, skipping any
/// from a group older than one already read: each group is a stream of its
/// own, so an older group can arrive last.
export async function* latestObjects(subscription: MoqSubscription): AsyncGenerator<ReceivedObject> {
  let newestGroupId = -1n
  for await (const object of subscription) {
    if (object.groupId > newestGroupId) {
      newestGroupId = object.groupId
      yield object
    }
  }
}
