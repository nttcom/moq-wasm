import { MoqSession } from 'moq-wasm'
import { connectOptions, withNamespaceRoot } from '../../../../utils/access'
import { LivePlayer } from 'moq-wasm/media'
import type { TelemetryStatsSample } from 'moq-wasm'
import { reportRelay, sampleCameras } from '../telemetry'
import type { CameraId } from '../types/monitoring'
import 'moq-wasm/media/player.css'

const log = (...args: unknown[]) => console.log('[mon][player]', ...args)

/// One Live Player per camera on the page's session. A camera's player draws
/// into a host element the page places on the stage or in the strip; the host
/// moves with the camera, so the player never notices the layout.
export class CameraPlayers {
  private session: MoqSession | undefined
  private readonly players = new Map<CameraId, { player: LivePlayer; trackNamespace: string }>()
  private readonly hosts = new Map<CameraId, HTMLDivElement>()

  constructor(private readonly onStateChange: (camId: CameraId) => void) {}

  async connect(relayUrl: string, onClosed: () => void): Promise<void> {
    reportRelay(relayUrl)
    const session = await MoqSession.connect(relayUrl, connectOptions())
    this.session = session
    sampleCameras(() => this.samples())
    void session.closed.then(() => {
      if (this.session === session) {
        onClosed()
      }
    })
    log('connected', { relayUrl })
  }

  host(camId: CameraId): HTMLDivElement {
    let host = this.hosts.get(camId)
    if (!host) {
      host = document.createElement('div')
      host.className = 'cam-host'
      this.hosts.set(camId, host)
    }
    return host
  }

  get(camId: CameraId): LivePlayer | undefined {
    return this.players.get(camId)?.player
  }

  async watch(location: string, camId: CameraId): Promise<void> {
    if (this.players.has(camId)) {
      return
    }
    if (!this.session) {
      throw new Error('not connected')
    }
    const player = new LivePlayer({
      container: this.host(camId),
      livePicture: 'canvas',
      keepLiveWhileReviewing: true,
      callbacks: {
        onStateChange: () => this.onStateChange(camId),
        onLiveFrame: () => {},
        onLog: (level, message) => log(level, { camId, message })
      }
    })
    const trackNamespace = withNamespaceRoot(['anon', location, camId])
    this.players.set(camId, { player, trackNamespace: trackNamespace.join('/') })
    try {
      await player.start(this.session, trackNamespace)
    } catch (error) {
      this.players.delete(camId)
      throw error
    }
  }

  private samples(): TelemetryStatsSample[] {
    return [...this.players.values()].flatMap(({ player, trackNamespace }) =>
      player.state.started ? [{ trackNamespace, values: player.stats() }] : []
    )
  }

  async disconnect(): Promise<void> {
    const session = this.session
    this.session = undefined
    for (const { player } of this.players.values()) {
      await player.stop()
    }
    this.players.clear()
    session?.close()
    log('disconnected')
  }
}
