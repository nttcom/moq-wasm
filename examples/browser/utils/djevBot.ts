import { FilterType, MoqtClientWrapper, RequestErrorCode } from '@moqt/moqtClient'
import { showBotOffHoursNotice } from './botHours'
import { DjevStatusView } from './djevStatus'
import { timeSeededGroupId } from './media/clock'
import { type StatusState, element, getErrorMessage, setStatus } from './page'
import { appendCloudRelayPresetButtons, configureRelayUrlControls } from './relayPresets'

const AUTH_INFO = ''
const EVENT_TIMELINE_TRACK = 'eventtimeline'

type BotSubscriber = { trackName: string; requestId: bigint; trackAlias: bigint }

export type DjevBotPageOptions = {
  session: MoqtClientWrapper
  logTag: string
  ownNamespace: string[]
  ownTracks: string[]
  botNamespace: string[]
  onSubscriber(subscriber: BotSubscriber): Promise<void> | void
  onUnsubscribe(requestId: bigint): void
  onTimelineObject(payload: Uint8Array): void
  onReset(): Promise<void> | void
  beforeConnect?(): Promise<void>
  afterConnect?(): Promise<void>
}

export function setUpDjevBotPage(options: DjevBotPageOptions): void {
  const { session, logTag, ownNamespace, ownTracks, botNamespace } = options
  const urlInput = element<HTMLInputElement>('url')
  const joinButton = element<HTMLButtonElement>('joinBtn')
  const leaveButton = element<HTMLButtonElement>('leaveBtn')
  const djevStatus = new DjevStatusView(element('djev-status'))
  showBotOffHoursNotice(element('bot-hours'))
  appendCloudRelayPresetButtons(element('urlPresets'))
  configureRelayUrlControls()

  const reset = async (connectionText: string, connectionState: StatusState): Promise<void> => {
    djevStatus.reset()
    await options.onReset()
    setStatus('connection-status', connectionText, connectionState)
    joinButton.disabled = false
    leaveButton.disabled = true
  }

  const subscribeTimeline = async (): Promise<void> => {
    try {
      const { subscribeOk } = await session.subscribe(botNamespace, EVENT_TIMELINE_TRACK, AUTH_INFO, {
        filterType: FilterType.LargestObject,
        forward: true
      })
      session.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (_groupId, object) => {
        if (object.objectStatus === undefined) {
          options.onTimelineObject(new Uint8Array(object.objectPayload))
        }
      })
    } catch (error) {
      console.warn(`[${logTag}] event timeline subscribe failed`, error)
    }
  }

  const join = async (): Promise<void> => {
    joinButton.disabled = true
    setStatus('connection-status', '接続中', 'review')
    try {
      await options.beforeConnect?.()
      await session.connect(urlInput.value.trim())
      session.setOnConnectionClosedHandler(() => void reset('切断されました', 'error'))
      session.setOnIncomingSubscribeHandler(async ({ subscribe, isSuccess, code, respondOk, respondError }) => {
        if (!isSuccess) {
          await respondError(BigInt(code), 'subscribe rejected')
          return
        }
        const { trackName, requestId } = subscribe
        if (!sameNamespace(subscribe.trackNamespace, ownNamespace) || !ownTracks.includes(trackName)) {
          await respondError(RequestErrorCode.TrackDoesNotExist, 'unknown track')
          return
        }
        const trackAlias = await respondOk()
        if (session.getRawClient()) {
          await options.onSubscriber({ trackName, requestId, trackAlias })
        }
      })
      session.setOnIncomingUnsubscribeHandler(options.onUnsubscribe)
      session.setOnPublishNamespaceHandler(async ({ publishNamespace, respondOk }) => {
        await respondOk()
        if (sameNamespace(publishNamespace.trackNamespace, botNamespace)) {
          await subscribeTimeline()
          await djevStatus
            .follow(session, botNamespace)
            .catch((error) => console.warn(`[${logTag}] djev status subscribe failed`, error))
        }
      })
      await session.publishNamespace(ownNamespace, AUTH_INFO)
      await session.subscribeNamespace(botNamespace, AUTH_INFO)
      await options.afterConnect?.()
      setStatus('connection-status', '接続済み', 'ok')
      leaveButton.disabled = false
    } catch (error) {
      await session.disconnect()
      await reset(`接続に失敗しました: ${getErrorMessage(error)}`, 'error')
    }
  }

  joinButton.addEventListener('click', () => void join())
  leaveButton.addEventListener('click', async () => {
    await session.disconnect()
    await reset('未接続', 'idle')
  })
}

/// One group per object: the bots read every group from its first object, and a
/// SUBSCRIBE that joins mid-group starts after it.
export class SingleObjectGroupPublisher {
  private nextGroupId = timeSeededGroupId()
  latestGroupId: bigint | undefined

  constructor(
    private readonly session: MoqtClientWrapper,
    readonly requestId: bigint,
    private readonly trackAlias: bigint
  ) {}

  reserveGroupId(): bigint {
    this.latestGroupId = this.nextGroupId++
    return this.latestGroupId
  }

  async send(payload: Uint8Array, groupId = this.reserveGroupId()): Promise<void> {
    await this.session.sendSingleObjectGroup(this.trackAlias, groupId, payload)
  }
}

/// draft-ietf-moq-msf-01 §8.1: each event timeline object is a JSON array of
/// records, newest last. The bots name the prompt group a record answers.
export function latestRecordForPrompt<T>(payload: Uint8Array, promptGroupId: bigint | undefined): T | undefined {
  const data = JSON.parse(new TextDecoder().decode(payload)).at(-1)?.data
  const [groupId] = Array.isArray(data?.prompt) ? data.prompt : []
  return Number.isInteger(groupId) && BigInt(groupId) === promptGroupId ? data : undefined
}

function sameNamespace(left: string[], right: string[]): boolean {
  return left.join('/') === right.join('/')
}
