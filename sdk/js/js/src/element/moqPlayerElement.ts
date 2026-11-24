import { sdkLog } from '../core/log'
import { type MoqConnectOptions, MoqSession } from '../core/session'
import { PlayerControls } from '../media/controls/playerControls'
import { getErrorMessage } from '../media/internal/status'
import { LivePlayer } from '../media/player/livePlayer'

const TARGET = 'moq-wasm::element::moq_player'

type Watch = { session: MoqSession; player: LivePlayer }

/// `<moq-player src="https://relay.example/moq" namespace="room/alice" token="..." controls>`
/// plays the broadcast of a `/`-separated track namespace, as `LivePlayer` does. It
/// connects once it is in the page with `src` and `namespace`, connects again when
/// `src`, `namespace` or the token changes, and closes the session when it is removed.
/// `controls` shows `PlayerControls`. A failed connect fires an `ErrorEvent` named `error`.
export class MoqPlayerElement extends HTMLElement {
  static readonly observedAttributes = ['src', 'namespace', 'token', 'controls']

  private tokenProperty: MoqConnectOptions['token']
  private watch: Watch | undefined
  private controlsPart: PlayerControls | undefined
  private work: Promise<void> = Promise.resolve()
  private reconnectQueued = false

  /// The `token` attribute, unless a token or a function that returns one was set
  /// here; the function is called before each CLIENT_SETUP.
  get token(): MoqConnectOptions['token'] {
    return this.tokenProperty ?? this.getAttribute('token') ?? undefined
  }

  set token(value: MoqConnectOptions['token']) {
    this.tokenProperty = value
    this.queueReconnect()
  }

  /// The `LivePlayer` while a session is open, `undefined` otherwise.
  get player(): LivePlayer | undefined {
    return this.watch?.player
  }

  connectedCallback(): void {
    this.queueReconnect()
  }

  disconnectedCallback(): void {
    this.queueReconnect()
  }

  attributeChangedCallback(name: string, oldValue: string | null, newValue: string | null): void {
    if (oldValue === newValue) {
      return
    }
    if (name === 'controls') {
      this.showControls()
    } else {
      this.queueReconnect()
    }
  }

  /// Attribute changes during a connect are applied once it settles, reading the
  /// attributes as they are then.
  private queueReconnect(): void {
    if (this.reconnectQueued) {
      return
    }
    this.reconnectQueued = true
    this.work = this.work.then(() => {
      this.reconnectQueued = false
      return this.reconnect()
    })
  }

  private async reconnect(): Promise<void> {
    await this.close()
    const src = this.getAttribute('src')
    const namespace = (this.getAttribute('namespace') ?? '').split('/').filter((part) => part.length > 0)
    if (!this.isConnected || !src || namespace.length === 0) {
      return
    }
    const player = new LivePlayer({
      container: this,
      callbacks: {
        onStateChange: () => this.controlsPart?.render(),
        onLiveFrame: () => this.controlsPart?.renderBuffer(player.stats()),
        onLog: () => {}
      }
    })
    let session: MoqSession | undefined
    try {
      session = await MoqSession.connect(src, { token: this.token })
      this.watch = { session, player }
      this.showControls()
      await player.start(session, namespace)
    } catch (error) {
      this.watch = undefined
      this.showControls()
      await player.dispose()
      session?.close()
      sdkLog(undefined, 'error', TARGET, 'moq-player failed to play', { url: src, error })
      this.dispatchEvent(new ErrorEvent('error', { error, message: getErrorMessage(error) }))
    }
  }

  private async close(): Promise<void> {
    const watch = this.watch
    if (!watch) {
      return
    }
    this.watch = undefined
    this.showControls()
    await watch.player.dispose()
    watch.session.close()
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
