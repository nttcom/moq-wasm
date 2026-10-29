import { FilterType, MoqtClientWrapper, RequestErrorCode } from '@moqt/moqtClient'
import { BOT_ROOM_END_TEXT, BotRoomRequest } from './botRoom'
import { SystemOneStatusView } from './systemOneStatus'
import { timeSeededGroupId } from './media/clock'
import { type StatusState, element, getErrorMessage, setStatus } from './page'
import { setUpRelayUrlControls } from './relayPresets'
import { type ServiceUrls, resolveServiceUrl } from './serviceUrl'

const AUTH_INFO = ''
const EVENT_TIMELINE_TRACK = 'eventtimeline'

type BotSubscriber = { trackName: string; requestId: bigint; trackAlias: bigint }

export type SystemOneBotPageOptions = {
  session: MoqtClientWrapper
  logTag: string
  ownNamespace: string[]
  ownTracks: string[]
  botNamespace: string[]
  /// The room the page asks the bot service to run a bot for while it is joined.
  room: string
  botService: ServiceUrls
  onSubscriber(subscriber: BotSubscriber): Promise<void> | void
  onUnsubscribe(requestId: bigint): void
  onTimelineObject(payload: Uint8Array): void
  onReset(): Promise<void> | void
  beforeConnect?(): Promise<void>
  afterConnect?(relayUrl: string): Promise<void>
}

export function setUpSystemOneBotPage(options: SystemOneBotPageOptions): void {
  const { session, logTag, ownNamespace, ownTracks, botNamespace } = options
  const urlInput = element<HTMLInputElement>('url')
  const joinButton = element<HTMLButtonElement>('joinBtn')
  const leaveButton = element<HTMLButtonElement>('leaveBtn')
  const systemOneStatus = new SystemOneStatusView(element('system-one-status'))
  const botRoomNotice = element<HTMLSpanElement>('bot-room')
  const botRoom = new BotRoomRequest((reason) => {
    botRoomNotice.textContent = BOT_ROOM_END_TEXT[reason]
    botRoomNotice.style.display = ''
  })
  botRoomNotice.style.display = 'none'
  setUpRelayUrlControls(urlInput, element('urlPresets'))

  const reset = async (connectionText: string, connectionState: StatusState): Promise<void> => {
    botRoom.close()
    systemOneStatus.reset()
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
    botRoomNotice.style.display = 'none'
    try {
      await options.beforeConnect?.()
      const relayUrl = urlInput.value.trim()
      // Asking before connecting lets the bot start while the page connects.
      botRoom.open(resolveServiceUrl(window.location.search, relayUrl, options.botService), options.room)
      await session.connect(relayUrl)
      session.setOnConnectionClosedHandler(() => void reset('切断されました', 'error'))
      session.setOnIncomingSubscribeHandler(async ({ subscribe, respondOk, respondError }) => {
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
          await systemOneStatus
            .follow(session, botNamespace)
            .catch((error) => console.warn(`[${logTag}] System One status subscribe failed`, error))
        }
      })
      await session.publishNamespace(ownNamespace, AUTH_INFO)
      await session.subscribeNamespace(botNamespace, AUTH_INFO)
      await options.afterConnect?.(relayUrl)
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
