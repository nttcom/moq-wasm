import type { MoqSession } from '../core/session'
import { followNamespaces } from './internal/followNamespaces'
import { RemoteBroadcast, type RemoteBroadcastOptions } from './remoteBroadcast'

/// What a callback throws is reported with `reportError`, as an exception of
/// an event listener is, and the watch goes on with the next callback and
/// namespace event. A broadcast stays played although its `onAdd` threw, and
/// stays removed although its `onRemove` threw.
export type WatchBroadcastsOptions = RemoteBroadcastOptions & {
  /// Asked once per announcement of a namespace other than this session's own
  /// broadcasts, whether to play it; a namespace it refuses or throws on is
  /// not played. Not asked on a withdrawal: a withdrawn namespace that is
  /// played is always removed.
  filter?: (namespace: string[]) => boolean
  onAdd(broadcast: RemoteBroadcast): void
  /// After the SDK disposed of the broadcast or failed to.
  onRemove(broadcast: RemoteBroadcast): void
}

export type BroadcastWatch = {
  /// UNSUBSCRIBE_NAMESPACE and disposes of every broadcast, without calling `onRemove`.
  close(): Promise<void>
}

/// Plays every broadcast published under `prefix` as a `RemoteBroadcast`,
/// those published before the call included, except the ones this session
/// published with `createBroadcast`. When a namespace is published again, as
/// by a participant who rejoined, what was played is removed, and it is played
/// anew only if `filter` accepts the new announcement. Resolves once the
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
