import { MoqSession } from 'moq-wasm'
import {
  type BroadcastWatch,
  type LocalBroadcast,
  type LocalSource,
  createBroadcast,
  watchBroadcasts
} from 'moq-wasm/media'
import { connectOptions, temporaryAppToken, withNamespaceRoot } from '../../utils/access'
import { BOT_ROOM_END_TEXT, BotRoomRequest, pageRoomId } from '../../utils/botRoom'
import { type StatusState, element, getErrorMessage, setStatus } from '../../utils/page'
import { startPageTelemetry } from '../../utils/pageTelemetry'
import { randomName } from '../../utils/randomName'
import { setUpRelayUrlControls } from '../../utils/relayPicker'
import { resolveServiceUrl } from '../../utils/serviceUrl'
import { type TranscriptTurn, transcriptTurns } from './transcript'

const ROOM = pageRoomId()
/// Must match examples/python/moq-ai-voice-agent/moq_ai_voice_agent/bot.py.
const ROOM_NAMESPACE = withNamespaceRoot(['anon', 'moq-ai-voice-agent', ROOM])
const BOT_NAMESPACE = [...ROOM_NAMESPACE, 'gemini']
const TRANSCRIPT_TRACK = 'transcript'
const BOT_SERVICE = {
  param: 'botUrl',
  local: 'http://127.0.0.1:8101',
  cloud: 'https://moq-ai-voice-agent.moqt.research.skyway.io'
}

const urlInput = element<HTMLInputElement>('url')
const joinButton = element<HTMLButtonElement>('joinBtn')
const leaveButton = element<HTMLButtonElement>('leaveBtn')
const muteButton = element<HTMLButtonElement>('muteBtn')
const transcriptList = element<HTMLOListElement>('transcript')
const transcriptEmpty = element<HTMLParagraphElement>('transcript-empty')
const botPlayer = element<HTMLDivElement>('bot-player')
const botRoomNotice = element<HTMLSpanElement>('bot-room')
const botRoom = new BotRoomRequest((reason) => {
  botRoomNotice.textContent = BOT_ROOM_END_TEXT[reason]
  botRoomNotice.style.display = ''
})

let session: MoqSession | undefined
let audioContext: AudioContext | undefined
let broadcast: LocalBroadcast | undefined
let microphone: LocalSource | undefined
let room: BroadcastWatch | undefined
let muted = false

startPageTelemetry('moq-ai-voice-agent', () => urlInput.value.trim())
setUpRelayUrlControls(urlInput, element('urlPresets'))
botRoomNotice.style.display = 'none'
joinButton.addEventListener('click', () => void join())
leaveButton.addEventListener('click', () => void leave('未接続', 'idle'))
muteButton.addEventListener('click', () => void toggleMute())

async function join(): Promise<void> {
  joinButton.disabled = true
  botRoomNotice.style.display = 'none'
  transcriptList.replaceChildren()
  transcriptEmpty.hidden = false
  setStatus('connection-status', '接続中', 'review')
  try {
    // Resumed from the click, as the browser allows only after a user gesture.
    audioContext = new AudioContext()
    await audioContext.resume()
    const relayUrl = urlInput.value.trim()
    // Asking before connecting lets the bot start while the page connects.
    botRoom.open(resolveServiceUrl(window.location.search, relayUrl, BOT_SERVICE), ROOM, { token: temporaryAppToken })
    const joined = await MoqSession.connect(relayUrl, connectOptions())
    session = joined
    void joined.closed.then(() => {
      if (session === joined) {
        void leave('切断されました', 'error')
      }
    })
    broadcast = await createBroadcast(joined, [...ROOM_NAMESPACE, randomName()], {
      onError: (error) => console.warn('[moq-ai-voice-agent] publish failed', error)
    })
    microphone = await broadcast.addSource('mic', {
      label: 'Mic',
      audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true }
    })
    room = await watchBroadcasts(joined, ROOM_NAMESPACE, {
      audioContext,
      filter: (namespace) => sameNamespace(namespace, BOT_NAMESPACE),
      onAdd: (remote) => {
        setBotStatus(true)
        remote.listen({ onSourceAdd: (view) => botPlayer.append(view.element) })
        void followTranscript(joined)
      },
      onRemove: () => setBotStatus(false)
    })
    setStatus('connection-status', '接続済み', 'ok')
    leaveButton.disabled = false
    muteButton.disabled = false
  } catch (error) {
    await leave(`接続に失敗しました: ${getErrorMessage(error)}`, 'error')
  }
}

async function leave(text: string, state: StatusState): Promise<void> {
  const leaving = session
  session = undefined
  botRoom.close()
  await room?.close().catch(() => undefined)
  room = undefined
  await broadcast?.close().catch(() => undefined)
  broadcast = undefined
  microphone = undefined
  leaving?.close()
  await audioContext?.close().catch(() => undefined)
  audioContext = undefined
  muted = false
  muteButton.textContent = 'ミュート'
  setBotStatus(false)
  setStatus('connection-status', text, state)
  joinButton.disabled = false
  leaveButton.disabled = true
  muteButton.disabled = true
}

async function toggleMute(): Promise<void> {
  if (!microphone) {
    return
  }
  muted = !muted
  await microphone.setMuted('audio', muted)
  muteButton.textContent = muted ? 'ミュート解除' : 'ミュート'
}

async function followTranscript(joined: MoqSession): Promise<void> {
  try {
    const subscription = await joined.subscribe(BOT_NAMESPACE, TRANSCRIPT_TRACK)
    for await (const object of subscription) {
      for (const turn of transcriptTurns(object.objectPayload)) {
        showTurn(turn)
      }
    }
  } catch (error) {
    console.warn('[moq-ai-voice-agent] transcript read ended', error)
  }
}

function showTurn(turn: TranscriptTurn): void {
  let item = turn.turn === undefined ? null : transcriptList.querySelector<HTMLLIElement>(`[data-turn="${turn.turn}"]`)
  if (!item) {
    item = document.createElement('li')
    item.className = 'voice-agent-turn'
    item.dataset.role = turn.role
    item.dataset.testid = 'voice-agent-turn'
    if (turn.turn !== undefined) {
      item.dataset.turn = String(turn.turn)
    }
    transcriptList.append(item)
  }
  item.textContent = turn.text
  transcriptEmpty.hidden = true
  item.scrollIntoView({ block: 'nearest' })
}

function setBotStatus(joined: boolean): void {
  setStatus('bot-status', joined ? 'bot 接続済み' : 'bot 未接続', joined ? 'ok' : 'idle')
}

function sameNamespace(left: string[], right: string[]): boolean {
  return left.join('/') === right.join('/')
}
