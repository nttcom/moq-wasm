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
        const broadcast = play(namespace)
        broadcasts.set(key, broadcast)
        onAdd(broadcast)
      }
    }
  } catch (error) {
    sdkLog(session, 'error', TARGET, 'watching broadcasts failed', { error: getErrorMessage(error) })
  }
}
