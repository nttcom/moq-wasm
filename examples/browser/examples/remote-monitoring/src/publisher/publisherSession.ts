import { MoqSession, type TrackWriter } from 'moq-wasm'
import type { CameraId } from '../types/monitoring'

const log = (...args: unknown[]) => console.log('[pub][session]', ...args)

export class PublisherSession {
  private session: MoqSession | undefined
  video: TrackWriter | undefined
  readonly namespace: string[]

  constructor(location: string, camId: CameraId) {
    this.namespace = ['anon', location, camId]
    log('created', { namespace: this.namespace })
  }

  async connect(relayUrl: string): Promise<void> {
    const session = await MoqSession.connect(relayUrl)
    this.session = session
    log('connected', { relayUrl })
    void session.closed.then(() => log('connection closed'))

    await session.announce(this.namespace)
    log('publishNamespace done', { namespace: this.namespace })
    const video = await session.publishTrack(this.namespace, 'video')
    this.video = video
    void (async () => {
      while (await video.nextSubscriber()) {
        log('subscriber joined')
      }
    })()
  }

  disconnect(): void {
    log('disconnecting...')
    this.session?.close()
    this.session = undefined
    this.video = undefined
    log('disconnected')
  }
}
