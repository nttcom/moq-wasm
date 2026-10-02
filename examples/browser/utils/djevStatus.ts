import type { MoqtClientWrapper } from '@moqt/moqtClient'

const STATUS_TRACK = 'status'
const AUTH_INFO = ''
const LARGEST_OBJECT_FILTER = 0x2
const CURRENT_GROUP = 0n

type DjevState = 'ready' | 'slow' | 'starting'

export class DjevStatusView {
  private state: DjevState = 'ready'
  private sinceMs = 0
  private shownGroupId = -1n
  private ticker: ReturnType<typeof setInterval> | undefined

  constructor(private readonly element: HTMLElement) {
    this.render()
  }

  async follow(session: MoqtClientWrapper, botNamespace: string[]): Promise<void> {
    const { requestId, subscribeOk } = await session.subscribe(botNamespace, STATUS_TRACK, AUTH_INFO, {
      filterType: LARGEST_OBJECT_FILTER,
      forward: true
    })
    session.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (groupId, object) => {
      if (object.objectStatus === undefined) {
        this.apply(groupId, object.objectPayload)
      }
    })
    if (subscribeOk.contentExists) {
      await session.relativeJoiningFetch(requestId, CURRENT_GROUP, {
        onObject: (object) => this.apply(object.groupId, object.objectPayload)
      })
    }
  }

  reset(): void {
    this.shownGroupId = -1n
    this.show('ready', 0)
  }

  private apply(groupId: bigint, payload: Uint8Array): void {
    if (groupId <= this.shownGroupId || payload.length === 0) {
      return
    }
    this.shownGroupId = groupId
    const { djev, since }: { djev: DjevState; since: number } = JSON.parse(new TextDecoder().decode(payload))
    this.show(djev, since)
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
