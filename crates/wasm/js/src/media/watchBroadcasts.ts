import type { MoqSession, MoqNamespaceWatch } from '../core/session'
import { sdkLog } from '../core/log'
import { getErrorMessage } from './internal/status'
import { isOwnBroadcast } from './ownBroadcasts'
import { RemoteBroadcast, type RemoteBroadcastOptions } from './remoteBroadcast'

const TARGET = 'moq-wasm::media::watch_broadcasts'

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
  void follow(session, watch, options, broadcasts, () => closed)
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

async function follow(
  session: MoqSession,
  watch: MoqNamespaceWatch,
  options: WatchBroadcastsOptions,
  broadcasts: Map<string, RemoteBroadcast>,
  closed: () => boolean
): Promise<void> {
  const { filter, onAdd, onRemove, ...broadcastOptions } = options
  const remove = async (key: string) => {
    const broadcast = broadcasts.get(key)
    if (!broadcast) {
      return
    }
    broadcasts.delete(key)
    await broadcast.dispose()
    onRemove(broadcast)
  }
  try {
    for await (const { kind, namespace } of watch) {
      if (closed()) {
        return
      }
      if (isOwnBroadcast(session, namespace) || (filter && !filter(namespace))) {
        continue
      }
      const key = JSON.stringify(namespace)
      await remove(key)
      if (kind === 'announced') {
        const broadcast = new RemoteBroadcast(session, namespace, broadcastOptions)
        broadcasts.set(key, broadcast)
        onAdd(broadcast)
      }
    }
  } catch (error) {
    sdkLog(session, 'error', TARGET, 'watching broadcasts failed', { error: getErrorMessage(error) })
  }
}
