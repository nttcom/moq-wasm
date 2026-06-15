import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { FetchObjectMessage, SubgroupObjectMessage } from '../../../../pkg/moqt_client_wasm'
import type { CameraId } from '../types/monitoring'

const log = (...args: unknown[]) => console.log('[mon][session]', ...args)

export class MonitorSession {
  private readonly client = new MoqtClientWrapper()

  async connect(relayUrl: string): Promise<void> {
    await this.client.connect(relayUrl)
    log('connected', { relayUrl })
    this.client.setOnConnectionClosedHandler(() => log('connection closed'))
  }

  async subscribeVideo(
    location: string,
    camId: CameraId,
    onObject: (groupId: bigint, msg: SubgroupObjectMessage) => void
  ): Promise<boolean> {
    const namespace = [location, camId]
    try {
      const { subscribeOk } = await this.client.subscribe(namespace, 'video', 'secret')
      log('subscribe OK', { camId, trackAlias: subscribeOk.trackAlias.toString() })
      this.client.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (groupId, msg) => onObject(groupId, msg))
      return true
    } catch (e) {
      log('subscribe failed', { camId, error: String(e) })
      return false
    }
  }

  async fetchVideo(
    location: string,
    camId: CameraId,
    startGroupId: bigint,
    endGroupId: bigint,
    onObject: (msg: FetchObjectMessage) => void
  ): Promise<bigint> {
    const namespace = [location, camId]
    const startedAt = Date.now()
    log('fetch start', {
      camId,
      startGroupId: startGroupId.toString(),
      endGroupId: endGroupId.toString()
    })
    const { requestId } = await this.client.fetch(namespace, 'video', startGroupId, 0n, endGroupId, 0n, { onObject })
    log('fetch OK', { camId, fetchId: requestId.toString(), elapsedMs: Date.now() - startedAt })
    return requestId
  }

  clearFetch(fetchId: bigint): void {
    this.client.clearFetchObjectHandler(fetchId)
  }

  async disconnect(): Promise<void> {
    log('disconnecting...')
    this.client.setOnConnectionClosedHandler(null)
    await this.client.finish()
    log('disconnected')
  }
}
