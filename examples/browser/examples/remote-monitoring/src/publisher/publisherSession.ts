import { MoqSession } from 'moq-wasm'
import { connectOptions, withNamespaceRoot } from '../../../../utils/access'
import { reportRelay } from '../telemetry'
import type { CameraId } from '../types/monitoring'

const log = (...args: unknown[]) => console.log('[pub][session]', ...args)

export class PublisherSession {
  session: MoqSession | undefined
  readonly namespace: string[]

  constructor(location: string, camId: CameraId) {
    this.namespace = withNamespaceRoot(['anon', location, camId])
    log('created', { namespace: this.namespace })
  }

  async connect(relayUrl: string): Promise<MoqSession> {
    reportRelay(relayUrl)
    const session = await MoqSession.connect(relayUrl, connectOptions())
    this.session = session
    log('connected', { relayUrl })
    void session.closed.then(() => log('connection closed'))
    return session
  }

  disconnect(): void {
    log('disconnecting...')
    this.session?.close()
    this.session = undefined
    log('disconnected')
  }
}
