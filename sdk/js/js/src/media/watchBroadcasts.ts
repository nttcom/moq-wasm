import type { MoqSession } from '../core/session'
import { followNamespaces } from './internal/followNamespaces'
import { RemoteBroadcast, type RemoteBroadcastOptions } from './remoteBroadcast'

export type WatchBroadcastsOptions = RemoteBroadcastOptions & {
  filter?: (namespace: string[]) => boolean
  onAdd(broadcast: RemoteBroadcast): void
  /// After the SDK disposed of the broadcast.
  onRemove(broadcast: RemoteBroadcast): void
}

export type BroadcastWatch = {
  /// UNSUBSCRIBE_NAMESPACE and disposes of every broadcast, without calling `onRemove`.
  close(): Promise<void>
}

/// Plays every broadcast published under `prefix` as a `RemoteBroadcast`,
/// those published before the call included, except the ones this session
/// published with `createBroadcast`. A namespace published again, as by a
/// participant who rejoined, is removed and added anew. Resolves once the
/// relay accepts the SUBSCRIBE_NAMESPACE, and rejects with its
/// `RequestError` otherwise.
export async function watchBroadcasts(
  session: MoqSession,
  prefix: string[],
  options: WatchBroadcastsOptions
): Promise<BroadcastWatch> {
  const watch = await session.watchNamespaces(prefix)
  const broadcasts = new Map<string, RemoteBroadcast>()
  let closed = false
  const { filter, onAdd, onRemove, ...broadcastOptions } = options
  const play = (namespace: string[]) => new RemoteBroadcast(session, namespace, broadcastOptions)
  void followNamespaces(session, watch, broadcasts, { filter, play, onAdd, onRemove }, () => closed)
  return {
    close: async () => {
      closed = true
      await watch.close()
      const all = [...broadcasts.values()]
      broadcasts.clear()
      await Promise.all(all.map((broadcast) => broadcast.dispose()))
    }
  }
}
