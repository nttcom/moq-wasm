import { MoqSession } from 'moq-wasm'
import type { CameraId } from '../types/monitoring'

const log = (...args: unknown[]) => console.log('[pub][session]', ...args)

export class PublisherSession {
  session: MoqSession | undefined
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
  }

  disconnect(): void {
    log('disconnecting...')
    this.session?.close()
    this.session = undefined
    log('disconnected')
  }
}
