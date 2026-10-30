import type { MoqtClientWrapper } from '../lib/moqt/moqtClient'

const STATUS_TRACK = 'status'
const AUTH_INFO = ''

type SystemOneState = 'ready' | 'slow' | 'starting'

export class SystemOneStatusView {
  private state: SystemOneState = 'ready'
  private sinceMs = 0
  private ticker: ReturnType<typeof setInterval> | undefined

  constructor(private readonly element: HTMLElement) {
    this.render()
  }

  async follow(session: MoqtClientWrapper, botNamespace: string[]): Promise<void> {
    await session.subscribeLatest(
      botNamespace,
      STATUS_TRACK,
      AUTH_INFO,
      (payload) => {
        const { system_one: state, since }: { system_one: SystemOneState; since: number } = JSON.parse(
          new TextDecoder().decode(payload)
        )
        this.show(state, since)
      },
      { onPublishDone: () => this.reset() }
    )
  }

  reset(): void {
    this.show('ready', 0)
  }

  private show(state: SystemOneState, sinceMs: number): void {
    this.state = state
    this.sinceMs = sinceMs
    clearInterval(this.ticker)
    this.ticker = state === 'ready' ? undefined : setInterval(() => this.render(), 1_000)
    this.render()
  }

  private render(): void {
    const elapsedSeconds = Math.max(0, Math.round((Date.now() - this.sinceMs) / 1_000))
    this.element.style.display = this.state === 'ready' ? 'none' : ''
    this.element.textContent =
      this.state === 'starting'
        ? `d1-2B を起動しています（経過 ${elapsedSeconds} 秒）`
        : this.state === 'slow'
          ? `d1-2B の応答に時間がかかっています（経過 ${elapsedSeconds} 秒）`
          : ''
  }
}
