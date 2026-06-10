import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { FetchObjectMessage, SubgroupObjectMessage } from '../../../../../pkg/moqt_client_wasm'
import type { CameraId } from '../types/monitoring'

const log = (...args: unknown[]) => console.log('[mon][session]', ...args)

export class MonitorSession {
  private readonly client = new MoqtClientWrapper()
  private nextSubscribeId = 1n
  private nextFetchId = 1n

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
    const subscribeId = this.nextSubscribeId++
    const namespace = [location, camId]
    try {
      const ok = await this.client.subscribe(subscribeId, namespace, 'video', 'secret')
      log('subscribe OK', { camId, trackAlias: ok.trackAlias.toString() })
      this.client.setOnSubgroupObjectHandler(ok.trackAlias, (groupId, msg) => onObject(groupId, msg))
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
    const fetchId = this.nextFetchId++
    const namespace = [location, camId]
    const startedAt = Date.now()
    log('fetch start', {
      camId,
      fetchId: fetchId.toString(),
      startGroupId: startGroupId.toString(),
      endGroupId: endGroupId.toString()
    })
    this.client.setOnFetchObjectHandler(fetchId, onObject)
    try {
      await this.client.fetch(fetchId, namespace, 'video', startGroupId, 0n, endGroupId, 0n)
      log('fetch OK', { camId, fetchId: fetchId.toString(), elapsedMs: Date.now() - startedAt })
      return fetchId
    } catch (e) {
      this.client.clearFetchObjectHandler(fetchId)
      throw e
    }
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
