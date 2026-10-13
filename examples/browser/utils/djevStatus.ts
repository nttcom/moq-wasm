import type { MoqtClientWrapper } from '@moqt/moqtClient'

const STATUS_TRACK = 'status'
const AUTH_INFO = ''

type DjevState = 'ready' | 'slow' | 'starting'

export class DjevStatusView {
  private state: DjevState = 'ready'
  private sinceMs = 0
  private ticker: ReturnType<typeof setInterval> | undefined

  constructor(private readonly element: HTMLElement) {
    this.render()
  }

  async follow(session: MoqtClientWrapper, botNamespace: string[]): Promise<void> {
    await session.subscribeLatest(botNamespace, STATUS_TRACK, AUTH_INFO, (payload) => {
      const { djev, since }: { djev: DjevState; since: number } = JSON.parse(new TextDecoder().decode(payload))
      this.show(djev, since)
    })
  }

  reset(): void {
    this.show('ready', 0)
  }

  private show(state: DjevState, sinceMs: number): void {
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
        ? `djev を起動しています（約 3 分、経過 ${elapsedSeconds} 秒）`
        : this.state === 'slow'
          ? `djev の応答に時間がかかっています（経過 ${elapsedSeconds} 秒）`
          : ''
  }
}
