import { MoqSession, type TrackWriter } from 'moq-wasm'
import { connectOptions } from './access'
import { RoomBotButton } from './roomBots'
import { SystemOneStatusView } from './systemOneStatus'
import { element, getErrorMessage, setStatus } from './page'
import { type StatusState, botNamespace, botTypeOf } from 'moq-wasm/media'
import { setUpRelayUrlControls } from './relayPicker'

const EVENT_TIMELINE_TRACK = 'eventtimeline'

export type SystemOneBotPageOptions = {
  logTag: string
  /// The room's namespace, `<app>/...`, which the bot button starts the bot in.
  room: string[]
  botType: string
  /// Called once connected, to announce the page's namespace and publish its tracks.
  onConnect(session: MoqSession): Promise<void> | void
  onTimelineObject(payload: Uint8Array): void
  onBotLeft(): void
  onReset(): Promise<void> | void
  beforeConnect?(): Promise<void>
  afterConnect?(session: MoqSession): Promise<void>
}

export function setUpSystemOneBotPage(options: SystemOneBotPageOptions): void {
  const { logTag } = options
  const ownBotNamespace = botNamespace(options.room, options.botType)
  const urlInput = element<HTMLInputElement>('url')
  const joinButton = element<HTMLButtonElement>('joinBtn')
  const leaveButton = element<HTMLButtonElement>('leaveBtn')
  const systemOneStatus = new SystemOneStatusView(element('system-one-status'))
  const botButton = new RoomBotButton(options.botType, options.room, element('botBtn'), element('bot-room'))
  setUpRelayUrlControls(urlInput, element('urlPresets'))
  let session: MoqSession | undefined

  const reset = async (connectionText: string, connectionState: StatusState): Promise<void> => {
    session = undefined
    botButton.setInRoom(false)
    systemOneStatus.reset()
    await options.onReset()
    setStatus('connection-status', connectionText, connectionState)
    joinButton.disabled = false
    leaveButton.disabled = true
  }

  const followBot = async (joined: MoqSession): Promise<void> => {
    const timeline = await joined.subscribe(ownBotNamespace, EVENT_TIMELINE_TRACK)
    void (async () => {
      for await (const object of timeline) {
        options.onTimelineObject(object.objectPayload)
      }
    })().catch((error: unknown) => console.warn(`[${logTag}] event timeline read ended`, error))
    await systemOneStatus.follow(joined, ownBotNamespace)
  }

  const join = async (): Promise<void> => {
    joinButton.disabled = true
    setStatus('connection-status', '接続中', 'review')
    try {
      await options.beforeConnect?.()
      const relayUrl = urlInput.value.trim()
      const joined = await MoqSession.connect(relayUrl, connectOptions())
      session = joined
      void joined.closed.then(() => {
        if (session === joined) {
          void reset('切断されました', 'error')
        }
      })
      await options.onConnect(joined)
      const bots = await joined.watchNamespaces(ownBotNamespace)
      void (async () => {
        for await (const { kind, namespace } of bots) {
          if (botTypeOf(options.room, namespace) !== options.botType) {
            continue
          }
          if (kind === 'withdrawn') {
            botButton.setBotPresent(false)
            options.onBotLeft()
          } else {
            botButton.setBotPresent(true)
            await followBot(joined).catch((error) => console.warn(`[${logTag}] bot subscribe failed`, error))
          }
        }
      })()
      await options.afterConnect?.(joined)
      setStatus('connection-status', '接続済み', 'ok')
      leaveButton.disabled = false
      botButton.setInRoom(true)
    } catch (error) {
      session?.close()
      await reset(`接続に失敗しました: ${getErrorMessage(error)}`, 'error')
    }
  }

  joinButton.addEventListener('click', () => void join())
  leaveButton.addEventListener('click', async () => {
    session?.close()
    await reset('未接続', 'idle')
  })
}

/// Calls `handler` each time a SUBSCRIBE for `writer`'s track is answered,
/// until the session ends. A handler error is logged and stops the calls.
export function onEachSubscriber(writer: TrackWriter, handler: () => Promise<void> | void): void {
  void (async () => {
    while (await writer.nextSubscriber()) {
      await handler()
    }
  })().catch((error: unknown) => console.warn('[systemOneBot] subscriber handler failed', error))
}

/// draft-ietf-moq-msf-02 §8.2: each event timeline object is a JSON array of
/// records, newest last. The bots name the prompt group a record answers.
export function latestRecordForPrompt<T>(payload: Uint8Array, promptGroupId: bigint | undefined): T | undefined {
  const data = JSON.parse(new TextDecoder().decode(payload)).at(-1)?.data
  const [groupId] = Array.isArray(data?.prompt) ? data.prompt : []
  return Number.isInteger(groupId) && BigInt(groupId) === promptGroupId ? data : undefined
}
