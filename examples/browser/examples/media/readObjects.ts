import type { MoqSubscription, ReceivedObject } from 'moq-wasm'

/// Reads the subgroup streams concurrently; unlike iterating `subscription`,
/// it keeps the status objects, which end a group.
export function readObjects(
  subscription: MoqSubscription,
  handler: (object: ReceivedObject) => void,
  logTag: string
): void {
  void (async () => {
    for await (const group of subscription.groups()) {
      void (async () => {
        for await (const object of group) {
          handler(object)
        }
      })().catch((error: unknown) => console.warn(`[${logTag}] group ${group.groupId} ended early`, error))
    }
  })().catch((error: unknown) => console.warn(`[${logTag}] read ended`, error))
}
