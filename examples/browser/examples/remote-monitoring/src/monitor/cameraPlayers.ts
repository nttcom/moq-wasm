import { MoqSession, type MoqNamespaceWatch } from 'moq-wasm'
import { connectOptions, withNamespaceRoot } from '../../../../utils/access'
import { LivePlayer } from 'moq-wasm/media'
import type { TelemetryStatsSample } from 'moq-wasm'
import { reportRelay, sampleCameras } from '../telemetry'
import type { CameraId } from '../types/monitoring'
import 'moq-wasm/media/player.css'

const log = (...args: unknown[]) => console.log('[mon][player]', ...args)

export type CameraEvent = { kind: 'added' | 'removed'; camId: CameraId }

/// One Live Player per camera on the page's session. A camera's player draws
/// into a host element the page places on the stage or in the strip; the host
/// moves with the camera, so the player never notices the layout.
///
/// The cameras of a location are the namespaces published under
/// `[root, location]`: `watchCameras` reports each one as it is announced and
/// starts its player, and stops the player when the namespace is withdrawn.
export class CameraPlayers {
  private session: MoqSession | undefined
  private watch: MoqNamespaceWatch | undefined
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

  /// SUBSCRIBE_NAMESPACE for the location. Every camera published under it,
  /// now or later, is reported once as `added` with its player started, and
  /// as `removed` once its publisher withdraws the namespace.
  async watchCameras(location: string, onEvent: (event: CameraEvent) => void): Promise<void> {
    if (!this.session) {
      throw new Error('not connected')
    }
    const prefix = withNamespaceRoot(['anon', location])
    const watch = await this.session.watchNamespaces(prefix)
    this.watch = watch
    void (async () => {
      for await (const { kind, namespace } of watch) {
        if (namespace.length !== prefix.length + 1) {
          log('namespace ignored', { namespace })
          continue
        }
        const camId = namespace[prefix.length]
        if (kind === 'announced') {
          if (this.players.has(camId)) {
            continue
          }
          try {
            await this.start(camId, namespace)
          } catch (error) {
            log('watch failed', { camId, error: String(error) })
            continue
          }
          onEvent({ kind: 'added', camId })
        } else {
          await this.stop(camId)
          onEvent({ kind: 'removed', camId })
        }
      }
    })().catch((error: unknown) => log('namespace watch ended', String(error)))
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

  private async start(camId: CameraId, trackNamespace: string[]): Promise<void> {
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
    this.players.set(camId, { player, trackNamespace: trackNamespace.join('/') })
    try {
      await player.start(this.session, trackNamespace)
    } catch (error) {
      this.players.delete(camId)
      throw error
    }
    log('watching', { camId })
  }

  private async stop(camId: CameraId): Promise<void> {
    const entry = this.players.get(camId)
    if (!entry) {
      return
    }
    this.players.delete(camId)
    await entry.player.stop()
    log('stopped', { camId })
  }

  private samples(): TelemetryStatsSample[] {
    return [...this.players.values()].flatMap(({ player, trackNamespace }) =>
      player.state.started ? [{ trackNamespace, values: player.stats() }] : []
    )
  }

  async disconnect(): Promise<void> {
    const session = this.session
    this.session = undefined
    await this.watch?.close()
    this.watch = undefined
    for (const { player } of this.players.values()) {
      await player.stop()
    }
    this.players.clear()
    session?.close()
    log('disconnected')
  }
}
