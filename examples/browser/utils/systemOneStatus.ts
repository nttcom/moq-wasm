import type { MoqSession } from 'moq-wasm'
import { latestObjects } from './latestObjects'

const STATUS_TRACK = 'status'

type SystemOneState = 'ready' | 'slow' | 'starting'

export class SystemOneStatusView {
  private state: SystemOneState = 'ready'
  private sinceMs = 0
  private ticker: ReturnType<typeof setInterval> | undefined

  constructor(private readonly element: HTMLElement) {
    this.render()
  }

  /// The bot writes one status object per group, so the current group holds the latest.
  async follow(session: MoqSession, botNamespace: string[]): Promise<void> {
    const status = await session.subscribeCurrentGroup(botNamespace, STATUS_TRACK)
    void status.done.then(() => this.reset())
    void (async () => {
      for await (const { objectPayload } of latestObjects(status)) {
        const { system_one: state, since }: { system_one: SystemOneState; since: number } = JSON.parse(
          new TextDecoder().decode(objectPayload)
        )
        this.show(state, since)
      }
    })().catch((error: unknown) => console.warn('[systemOneStatus] status read ended', error))
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
