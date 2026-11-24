import type { MoqNamespaceWatch, MoqSession } from '../../core/session'
import { sdkLog } from '../../core/log'
import { isOwnBroadcast } from '../ownBroadcasts'
import { getErrorMessage } from './status'

const TARGET = 'moq-wasm::media::watch_broadcasts'

export type FollowNamespacesOptions<B> = {
  filter?: (namespace: string[]) => boolean
  play(namespace: string[]): B
  onAdd(broadcast: B): void
  onRemove(broadcast: B): void
}

/// Calls a callback of the page. What it throws is the page's bug, reported
/// as an event listener's would be, and the watch goes on; `undefined` then.
function callPage<T>(callback: () => T): T | undefined {
  try {
    return callback()
  } catch (error) {
    reportError(error)
    return undefined
  }
}

/// Takes `play` rather than making the `RemoteBroadcast` itself, so that it
/// runs where the wasm module cannot load, as in the unit tests.
export async function followNamespaces<B extends { dispose(): Promise<void> }>(
  session: MoqSession,
  watch: MoqNamespaceWatch,
  broadcasts: Map<string, B>,
  options: FollowNamespacesOptions<B>,
  closed: () => boolean
): Promise<void> {
  const { filter, play, onAdd, onRemove } = options
  const remove = async (namespace: string[], key: string) => {
    const broadcast = broadcasts.get(key)
    if (!broadcast) {
      return
    }
    broadcasts.delete(key)
    try {
      await broadcast.dispose()
    } catch (error) {
      sdkLog(session, 'error', TARGET, 'disposing of a broadcast failed', {
        track_namespace: namespace.join('/'),
        error: getErrorMessage(error)
      })
    }
    // `close()` during the disposal took the page's view down without `onRemove`.
    if (!closed()) {
      callPage(() => onRemove(broadcast))
    }
  }
  const add = (namespace: string[], key: string) => {
    let broadcast: B
    try {
      broadcast = play(namespace)
    } catch (error) {
      sdkLog(session, 'error', TARGET, 'playing a broadcast failed', {
        track_namespace: namespace.join('/'),
        error: getErrorMessage(error)
      })
      return
    }
    broadcasts.set(key, broadcast)
    callPage(() => onAdd(broadcast))
  }
  try {
    for await (const { kind, namespace } of watch) {
      if (closed()) {
        return
      }
      const key = JSON.stringify(namespace)
      await remove(namespace, key)
      // `close()` disposes of only what the map holds, so nothing is played
      // after it, called during the disposal or by `filter`.
      if (closed()) {
        return
      }
      if (kind !== 'announced' || isOwnBroadcast(session, namespace)) {
        continue
      }
      if (filter && !callPage(() => filter(namespace))) {
        continue
      }
      if (closed()) {
        return
      }
      add(namespace, key)
    }
  } catch (error) {
    sdkLog(session, 'error', TARGET, 'watching broadcasts failed', { error: getErrorMessage(error) })
  }
}
