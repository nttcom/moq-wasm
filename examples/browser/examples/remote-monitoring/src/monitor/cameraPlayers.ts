import { MoqSession } from 'moq-wasm'
import { connectOptions, withNamespaceRoot } from '../../../../utils/access'
import {
  type BroadcastWatch,
  type LivePlayer,
  type RemoteBroadcast,
  type SourceView,
  watchBroadcasts
} from 'moq-wasm/media'
import type { TelemetryStatsSample } from 'moq-wasm'
import { reportRelay, sampleCameras } from '../telemetry'
import type { CameraId } from '../types/monitoring'
import 'moq-wasm/media/player.css'

const log = (...args: unknown[]) => console.log('[mon][player]', ...args)

export type CameraEvent = { kind: 'added' | 'removed'; camId: CameraId }

/// The cameras of a location are the broadcasts published under `[root, location]`, and a camera
/// is the first source of its broadcast. Its view sits in a host element the page places on the
/// stage or in the strip; the host moves with the camera, so the player never notices the layout.
export class CameraPlayers {
  private session: MoqSession | undefined
  private watch: BroadcastWatch | undefined
  private readonly views = new Map<CameraId, { view: SourceView; trackNamespace: string }>()
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

  /// Every camera published under the location, now or later, is reported as `added` once its
  /// catalog lists a source, and as `removed` once the source or the broadcast is gone.
  async watchCameras(location: string, onEvent: (event: CameraEvent) => void): Promise<void> {
    if (!this.session) {
      throw new Error('not connected')
    }
    const prefix = withNamespaceRoot(['anon', location])
    const remove = (camId: CameraId) => {
      if (this.views.delete(camId)) {
        log('stopped', { camId })
        onEvent({ kind: 'removed', camId })
      }
    }
    this.watch = await watchBroadcasts(this.session, prefix, {
      keepLiveWhileReviewing: true,
      filter: (namespace) => namespace.length === prefix.length + 1,
      onLog: (level, message) => log(level, message),
      onAdd: (broadcast: RemoteBroadcast) => {
        const camId = broadcast.namespace[prefix.length]
        const show = (view: SourceView) => {
          this.views.set(camId, { view, trackNamespace: broadcast.namespace.join('/') })
          this.host(camId).replaceChildren(view.element)
          view.listen({ onPlayerChange: () => this.onStateChange(camId) })
          log('watching', { camId })
          onEvent({ kind: 'added', camId })
        }
        broadcast.listen({
          onSourceAdd: (view) => {
            if (!this.views.has(camId)) {
              show(view)
            }
          },
          // A source that replaced this one was added before this one finished leaving.
          onSourceRemove: (view) => {
            if (this.views.get(camId)?.view !== view) {
              return
            }
            remove(camId)
            const [next] = broadcast.sources.values()
            if (next) {
              show(next)
            }
          }
        })
      },
      onRemove: (broadcast) => remove(broadcast.namespace[prefix.length])
    })
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
    return this.views.get(camId)?.view.player
  }

  private samples(): TelemetryStatsSample[] {
    return [...this.views.values()].flatMap(({ view, trackNamespace }) =>
      view.player.state.started ? [{ trackNamespace, values: view.stats() }] : []
    )
  }

  async disconnect(): Promise<void> {
    const session = this.session
    this.session = undefined
    await this.watch?.close()
    this.watch = undefined
    this.views.clear()
    session?.close()
    log('disconnected')
  }
}
