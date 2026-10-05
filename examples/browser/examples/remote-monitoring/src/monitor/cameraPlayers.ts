import { MoqtClientWrapper } from '@moqt/moqtClient'
import { LivePlayer } from '@player/livePlayer'
import type { CameraId } from '../types/monitoring'

const CAMERA_VIDEO_CODEC = 'avc3.640028'
const AUTH_INFO = 'secret'

const log = (...args: unknown[]) => console.log('[mon][player]', ...args)

/// One Live Player per camera on the page's session. A camera's player draws
/// into a host element the page places on the stage or in the strip; the host
/// moves with the camera, so the player never notices the layout.
export class CameraPlayers {
  private readonly client = new MoqtClientWrapper()
  private readonly players = new Map<CameraId, LivePlayer>()
  private readonly hosts = new Map<CameraId, HTMLDivElement>()

  constructor(private readonly onStateChange: (camId: CameraId) => void) {}

  async connect(relayUrl: string, onClosed: () => void): Promise<void> {
    await this.client.connect(relayUrl)
    this.client.setOnConnectionClosedHandler(onClosed)
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
    return this.players.get(camId)
  }

  async watch(location: string, camId: CameraId): Promise<void> {
    if (this.players.has(camId)) {
      return
    }
    const player = new LivePlayer({
      client: this.client,
      container: this.host(camId),
      livePicture: 'canvas',
      callbacks: {
        onStateChange: () => this.onStateChange(camId),
        onLiveFrame: () => {},
        onLog: (level, message) => log(level, { camId, message })
      }
    })
    this.players.set(camId, player)
    try {
      await player.start(['anon', location, camId], AUTH_INFO, {
        video: [{ name: 'video', label: camId, codec: CAMERA_VIDEO_CODEC }]
      })
    } catch (error) {
      this.players.delete(camId)
      throw error
    }
  }

  async disconnect(): Promise<void> {
    this.client.setOnConnectionClosedHandler(null)
    for (const player of this.players.values()) {
      await player.stop()
    }
    this.players.clear()
    await this.client.finish()
    log('disconnected')
  }
}
