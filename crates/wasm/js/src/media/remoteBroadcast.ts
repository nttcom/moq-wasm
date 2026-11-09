import type { MoqSession } from '../core/session'
import { sdkLog } from '../core/log'
import { extractCatalogTracks } from './catalog'
import { parseCatalog } from './parseCatalog'
import type { AudioLevel } from './internal/loc'
import { getErrorMessage } from './internal/status'
import { BroadcastCatalog } from './player/broadcastCatalog'
import { LivePlayer, type LivePlayerStats } from './player/livePlayer'
import type { LogLevel } from './player/trackContext'
import { type CatalogSource, catalogSources, liveSource } from './sourceNames'

const TARGET = 'moq-wasm::media::remote_broadcast'
const VIDEO_SIZE_CHECK_MS = 500

export type SourceViewState = {
  /// `waiting` until a frame of the subscribed video is shown, `off` while the
  /// catalog lists no video of the source or the page turned it off.
  video: 'waiting' | 'playing' | 'off'
  /// `off` while the catalog lists no audio of the source.
  audio: 'playing' | 'off'
  /// Absent until a frame is shown.
  videoSize?: { width: number; height: number }
}

export type SourceViewListener = {
  onChange?(view: SourceView): void
  onPlayerChange?(view: SourceView): void
}

export interface SourceView {
  readonly name: string
  /// The catalog `label` of the source's video, or of its audio without one.
  readonly label: string
  /// Created by the SDK, which removes it when the source leaves the catalog.
  /// The page places it and may move it to another parent within one task
  /// without the picture stopping.
  readonly element: HTMLElement
  readonly state: SourceViewState
  /// The source's player, for the operations a view does not carry, such as
  /// seeking or choosing a track.
  readonly player: LivePlayer
  /// Returns the function that removes the listener.
  listen(listener: SourceViewListener): () => void
  setVolume(volume: number): void
  stats(): LivePlayerStats
  /// `false` unsubscribes the video and keeps the audio. Showing the view
  /// again does not turn back on a video the page turned off.
  setVideoEnabled(enabled: boolean): Promise<void>
}

export type RemoteBroadcastListener = {
  onSourceAdd?(view: SourceView): void
  /// After the SDK disposed of the view's player and removed its element.
  onSourceRemove?(view: SourceView): void
  /// The LOC Audio Level (draft-ietf-moq-loc-04 §2.3.3.2) of each live audio
  /// object that carries one.
  onAudioLevel?(view: SourceView, level: AudioLevel): void
}

export type RemoteBroadcastOptions = {
  /// Receives the logs of the broadcast's catalog and players instead of the console.
  onLog?: (level: LogLevel, message: string) => void
  /// Unsubscribes the video of a view whose element is off screen, detached
  /// or hidden, until it shows again; the audio plays on. On by default.
  pauseHiddenVideo?: boolean
  /// Plays the views' live sound through the page's context, as
  /// `LivePlayerOptions.audioContext` does: the page resumes it from a user
  /// gesture and picks the speaker with its `setSinkId`.
  audioContext?: AudioContext
}

/// A broadcast someone else publishes, played source by source: it
/// subscribes to the catalog once and creates a view for each source the
/// catalog lists (see `catalogSources`), until `dispose`.
export class RemoteBroadcast {
  private readonly views = new Map<string, View>()
  private readonly listeners = new Set<RemoteBroadcastListener>()
  private readonly broadcastCatalog: BroadcastCatalog
  private readonly unlistenCatalog: () => void
  private readonly log: (level: LogLevel, message: string) => void
  private readonly pauseHiddenVideo: boolean
  private readonly audioContext: AudioContext | undefined
  private disposed = false

  constructor(
    readonly session: MoqSession,
    readonly namespace: string[],
    options: RemoteBroadcastOptions = {}
  ) {
    this.log = options.onLog ?? ((level, message) => sdkLog(level, TARGET, message))
    this.pauseHiddenVideo = options.pauseHiddenVideo ?? true
    this.audioContext = options.audioContext
    this.broadcastCatalog = new BroadcastCatalog(session, namespace, { onLog: options.onLog })
    this.unlistenCatalog = this.broadcastCatalog.listen({
      onCatalog: (text) => this.applyCatalog(text),
      onPublisherAbsent: () => this.applySources([])
    })
    this.broadcastCatalog.start().catch((error: unknown) => this.log('error', `catalog: ${getErrorMessage(error)}`))
  }

  get sources(): ReadonlyMap<string, SourceView> {
    return this.views
  }

  /// Owned by the broadcast: `dispose` closes it, so do not close it yourself.
  get catalog(): BroadcastCatalog {
    return this.broadcastCatalog
  }

  /// Tells `onSourceAdd` of every source there is before returning. Returns
  /// the function that removes the listener.
  listen(listener: RemoteBroadcastListener): () => void {
    this.listeners.add(listener)
    for (const view of this.views.values()) {
      listener.onSourceAdd?.(view)
    }
    return () => this.listeners.delete(listener)
  }

  /// Disposes of the views and unsubscribes the catalog. The session stays open.
  async dispose(): Promise<void> {
    this.disposed = true
    this.unlistenCatalog()
    const views = [...this.views.values()]
    this.views.clear()
    await Promise.all(views.map((view) => view.dispose()))
    await this.broadcastCatalog.close()
  }

  private applyCatalog(text: string): void {
    let catalog: unknown
    try {
      catalog = parseCatalog(text)
    } catch (error) {
      this.log('warn', `catalog ignored: ${getErrorMessage(error)}`)
      return
    }
    this.applySources(catalogSources(extractCatalogTracks(catalog, 'video'), extractCatalogTracks(catalog, 'audio')))
  }

  private applySources(sources: CatalogSource[]): void {
    if (this.disposed) {
      return
    }
    const listed = new Map(sources.map((source) => [source.name, source]))
    for (const [name, view] of this.views) {
      if (!listed.has(name)) {
        this.views.delete(name)
        void view.dispose().then(() => {
          for (const listener of this.listeners) {
            listener.onSourceRemove?.(view)
          }
        })
      }
    }
    for (const source of sources) {
      const existing = this.views.get(source.name)
      if (existing) {
        existing.update(source)
        continue
      }
      const view = new View(source, this.log, this.pauseHiddenVideo, this.audioContext, (level) =>
        this.audioLevel(view, level)
      )
      this.views.set(source.name, view)
      view.player
        .start(this.session, this.namespace, this.catalog)
        .catch((error: unknown) => this.log('error', `source ${source.name}: ${getErrorMessage(error)}`))
      for (const listener of this.listeners) {
        listener.onSourceAdd?.(view)
      }
    }
  }

  private audioLevel(view: View, level: AudioLevel): void {
    for (const listener of this.listeners) {
      listener.onAudioLevel?.(view, level)
    }
  }
}

class View implements SourceView {
  readonly element = document.createElement('div')
  readonly player: LivePlayer
  readonly name: string
  label: string
  state: SourceViewState
  private readonly listeners = new Set<SourceViewListener>()
  private listed: { video: boolean; audio: boolean }
  private videoEnabled = true
  private shown = true
  private readonly visibility: IntersectionObserver | undefined
  private videoSizeCheckedAtMs = Number.NEGATIVE_INFINITY

  constructor(
    source: CatalogSource,
    log: (level: LogLevel, message: string) => void,
    pauseHiddenVideo: boolean,
    audioContext: AudioContext | undefined,
    onAudioLevel: (level: AudioLevel) => void
  ) {
    this.name = source.name
    this.label = source.label
    this.listed = { video: source.video !== undefined, audio: source.audio !== undefined }
    this.state = { video: this.listed.video ? 'waiting' : 'off', audio: this.listed.audio ? 'playing' : 'off' }
    this.element.className = 'moq-source'
    this.element.dataset.source = source.name
    this.player = new LivePlayer({
      container: this.element,
      source: liveSource(source),
      audioContext,
      callbacks: {
        onStateChange: () => {
          for (const listener of this.listeners) {
            listener.onPlayerChange?.(this)
          }
        },
        onLiveFrame: () => this.framePresented(),
        onLog: log,
        onAudioLevel
      }
    })
    this.showState()
    if (pauseHiddenVideo) {
      // No video until the element shows, so a view the page never places subscribes none.
      this.shown = false
      void this.player.setVideoEnabled(false)
      this.visibility = new IntersectionObserver(([entry]) => void this.setShown(entry.isIntersecting))
      this.visibility.observe(this.element)
    }
  }

  listen(listener: SourceViewListener): () => void {
    this.listeners.add(listener)
    return () => this.listeners.delete(listener)
  }

  setVolume(volume: number): void {
    this.player.setVolume(volume)
  }

  stats(): LivePlayerStats {
    return this.player.stats()
  }

  async setVideoEnabled(enabled: boolean): Promise<void> {
    this.videoEnabled = enabled
    this.refresh()
    await this.player.setVideoEnabled(enabled && this.shown)
  }

  private async setShown(shown: boolean): Promise<void> {
    if (shown === this.shown) {
      return
    }
    this.shown = shown
    // The picture shows a stale frame until the video subscribed again delivers one.
    if (shown && this.state.video === 'playing') {
      this.setState({ ...this.state, video: 'waiting' })
    }
    await this.player.setVideoEnabled(this.videoEnabled && shown)
  }

  update(source: CatalogSource): void {
    this.label = source.label
    this.listed = { video: source.video !== undefined, audio: source.audio !== undefined }
    this.refresh()
  }

  async dispose(): Promise<void> {
    this.listeners.clear()
    this.visibility?.disconnect()
    await this.player.dispose()
    this.element.remove()
  }

  private refresh(): void {
    const videoOn = this.listed.video && this.videoEnabled
    const video = !videoOn ? 'off' : this.state.video === 'off' ? 'waiting' : this.state.video
    this.setState({ ...this.state, video, audio: this.listed.audio ? 'playing' : 'off' })
  }

  private framePresented(): void {
    const nowMs = performance.now()
    if (this.state.video === 'playing' && nowMs - this.videoSizeCheckedAtMs < VIDEO_SIZE_CHECK_MS) {
      return
    }
    this.videoSizeCheckedAtMs = nowMs
    if (this.state.video === 'off') {
      return
    }
    const videoSize = this.player.stats().frameSize ?? this.state.videoSize
    const sameSize =
      videoSize?.width === this.state.videoSize?.width && videoSize?.height === this.state.videoSize?.height
    if (this.state.video === 'playing' && sameSize) {
      return
    }
    this.setState({ ...this.state, video: 'playing', videoSize })
  }

  private setState(state: SourceViewState): void {
    const changed =
      state.video !== this.state.video ||
      state.audio !== this.state.audio ||
      state.videoSize?.width !== this.state.videoSize?.width ||
      state.videoSize?.height !== this.state.videoSize?.height
    this.state = state
    if (!changed) {
      return
    }
    this.showState()
    for (const listener of this.listeners) {
      listener.onChange?.(this)
    }
  }

  private showState(): void {
    this.element.dataset.video = this.state.video
    this.element.dataset.audio = this.state.audio
  }
}
