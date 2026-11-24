import { sdkLog } from '../core/log'
import { type MoqConnectOptions, MoqSession } from '../core/session'
import { PlayerControls } from '../media/controls/playerControls'
import { getErrorMessage } from '../media/internal/status'
import { LivePlayer, type LiveTracks } from '../media/player/livePlayer'
import type { Packaging } from '../media/player/trackContext'

const TARGET = 'moq-wasm::element::moq_player'

/// `ownedSession` is the session the element opened from `src`, which it closes.
type Watch = { player: LivePlayer; ownedSession: MoqSession | undefined }

/// `<moq-player src="https://relay.example/moq" namespace="room/alice" token="..." controls>`
/// plays the broadcast of a `/`-separated track namespace, as `LivePlayer` does. It
/// plays once it is in the page with `namespace` and either `src` or a `session`, starts
/// again when `src`, `namespace`, the token, `session` or `tracks` changes, and stops when
/// it is removed. `controls` shows `PlayerControls`, and `packaging="cmaf"` plays the CMAF
/// tracks. It fires `statechange` and `liveframe` as `LivePlayerCallbacks` are called, and
/// an `ErrorEvent` named `error` when it fails to start.
export class MoqPlayerElement extends HTMLElement {
  static readonly observedAttributes = ['src', 'namespace', 'token', 'controls', 'packaging']

  private tokenProperty: MoqConnectOptions['token']
  private sessionProperty: MoqSession | undefined
  private tracksProperty: LiveTracks | undefined
  private watch: Watch | undefined
  private controlsPart: PlayerControls | undefined
  private work: Promise<void> = Promise.resolve()
  private restartQueued = false

  /// The `token` attribute, unless a token or a function that returns one was set
  /// here; the function is called before each CLIENT_SETUP.
  get token(): MoqConnectOptions['token'] {
    return this.tokenProperty ?? this.getAttribute('token') ?? undefined
  }

  set token(value: MoqConnectOptions['token']) {
    this.tokenProperty = value
    this.queueRestart()
  }

  /// A session the page opened, played on instead of connecting to `src`; the
  /// element never closes it.
  get session(): MoqSession | undefined {
    return this.sessionProperty
  }

  set session(value: MoqSession | undefined) {
    this.sessionProperty = value
    this.queueRestart()
  }

  /// The tracks to play instead of those the catalog lists, as in `LivePlayer.start`.
  get tracks(): LiveTracks | undefined {
    return this.tracksProperty
  }

  set tracks(value: LiveTracks | undefined) {
    this.tracksProperty = value
    this.queueRestart()
  }

  /// The `LivePlayer` while it plays, `undefined` otherwise.
  get player(): LivePlayer | undefined {
    return this.watch?.player
  }

  connectedCallback(): void {
    this.queueRestart()
  }

  disconnectedCallback(): void {
    this.queueRestart()
  }

  attributeChangedCallback(name: string, oldValue: string | null, newValue: string | null): void {
    if (oldValue === newValue) {
      return
    }
    if (name === 'controls') {
      this.showControls()
    } else if (name === 'packaging') {
      void this.watch?.player.setPackaging(this.packaging())
    } else {
      this.queueRestart()
    }
  }

  /// Changes made while it starts are applied once it settles, reading the
  /// attributes and properties as they are then.
  private queueRestart(): void {
    if (this.restartQueued) {
      return
    }
    this.restartQueued = true
    this.work = this.work.then(() => {
      this.restartQueued = false
      return this.restart()
    })
  }

  private async restart(): Promise<void> {
    await this.stop()
    const source = this.sessionProperty ?? (this.getAttribute('src') || undefined)
    const namespace = (this.getAttribute('namespace') ?? '').split('/').filter((part) => part.length > 0)
    if (!this.isConnected || !source || namespace.length === 0) {
      return
    }
    const player = new LivePlayer({
      container: this,
      callbacks: {
        onStateChange: () => {
          this.controlsPart?.render()
          this.dispatchEvent(new Event('statechange'))
        },
        onLiveFrame: () => {
          this.controlsPart?.renderBuffer(player.stats())
          this.dispatchEvent(new Event('liveframe'))
        },
        onLog: () => {}
      }
    })
    let ownedSession: MoqSession | undefined
    try {
      const session =
        typeof source === 'string' ? (ownedSession = await MoqSession.connect(source, { token: this.token })) : source
      this.watch = { player, ownedSession }
      this.showControls()
      await player.setPackaging(this.packaging())
      await player.start(session, namespace, this.tracksProperty)
    } catch (error) {
      this.watch = undefined
      this.showControls()
      await player.dispose()
      ownedSession?.close()
      sdkLog(undefined, 'error', TARGET, 'moq-player failed to play', {
        url: typeof source === 'string' ? source : undefined,
        error
      })
      this.dispatchEvent(new ErrorEvent('error', { error, message: getErrorMessage(error) }))
    }
  }

  private async stop(): Promise<void> {
    const watch = this.watch
    if (!watch) {
      return
    }
    this.watch = undefined
    this.showControls()
    await watch.player.dispose()
    watch.ownedSession?.close()
  }

  private packaging(): Packaging {
    return this.getAttribute('packaging') === 'cmaf' ? 'cmaf' : 'loc'
  }

  private showControls(): void {
    const player = this.watch?.player
    const shown = player !== undefined && this.hasAttribute('controls')
    if (shown === (this.controlsPart !== undefined)) {
      return
    }
    if (player && shown) {
      this.controlsPart = new PlayerControls(this, player, () => {})
      this.controlsPart.render()
    } else {
      this.controlsPart?.dispose()
      this.controlsPart = undefined
    }
  }
}
