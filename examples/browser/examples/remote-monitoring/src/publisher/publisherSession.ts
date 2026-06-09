import { MoqtClientWrapper } from '@moqt/moqtClient'
import type { MOQTClient } from '../../../../../pkg/moqt_client_wasm'
import type { CameraId } from '../types/monitoring'

const log = (...args: unknown[]) => console.log('[pub][session]', ...args)

export class PublisherSession {
  private readonly client = new MoqtClientWrapper()
  readonly namespace: string[]

  constructor(location: string, camId: CameraId) {
    this.namespace = [location, camId]
    log('created', { namespace: this.namespace })
  }

  async connect(relayUrl: string): Promise<void> {
    log('connecting...', { relayUrl })
    await this.client.connect(relayUrl)
    log('connected')

    log('publishNamespace...', { namespace: this.namespace })
    await this.client.publishNamespace(this.namespace, 'secret')
    log('publishNamespace done')

    this.client.setOnIncomingSubscribeHandler(async ({ subscribe, respondOk, respondError }) => {
      log('incoming SUBSCRIBE', {
        trackName: subscribe.trackName,
        requestId: subscribe.requestId?.toString()
      })
      if (subscribe.trackName === 'video') {
        const trackAlias = await respondOk()
        log('responded OK', { trackAlias: trackAlias.toString() })
        // verify getTrackSubscribers sees this subscriber
        const aliases = this.getVideoTrackAliases()
        log('getVideoTrackAliases after respondOk', { aliases: aliases.map((a) => a.toString()) })
      } else {
        log('rejected unknown track', { trackName: subscribe.trackName })
        await respondError(0n, 'unknown track')
      }
    })

    this.client.setOnConnectionClosedHandler(() => {
      log('connection closed')
    })
  }

  getVideoTrackAliases(): bigint[] {
    const raw = this.getRawClient()
    return Array.from(raw.getTrackSubscribers(this.namespace, 'video'), (v) => BigInt(v))
  }

  getRawClient(): MOQTClient {
    return this.client.getRawClient()
  }

  async disconnect(): Promise<void> {
    log('disconnecting...')
    this.client.setOnIncomingSubscribeHandler(null)
    this.client.setOnConnectionClosedHandler(null)
    await this.client.finish()
    log('disconnected')
  }
}
