import { MoqSession } from 'moq-wasm'
import { PhoneCall, type PhoneCallState } from 'moq-wasm/media'
import { offeredVersions } from '../../utils/moqtVersion'
import { element, getErrorMessage } from '../../utils/page'
import { pageRoomId } from '../../utils/roomId'
import { setUpRelayUrlControls } from '../../utils/relayPicker'
import { randomName } from '../../utils/randomName'
import { urlFromQuery } from '../../utils/serviceUrl'
import { startPageTelemetry } from '../../utils/pageTelemetry'

const DEFAULT_APP_SERVER_URL = 'http://127.0.0.1:8097'
const AUDIO_STATS_MS = 1_000

type Incoming = { call_id: string; from: string; to: string }
type Joined = { session: MoqSession; namespace: string[] }

const STATE_LABELS: Record<PhoneCallState, string> = {
  dialing: '発信中',
  ringing: '呼び出し中',
  answered: '通話中',
  ended: '終了'
}

const urlInput = element<HTMLInputElement>('url')
const joinButton = element<HTMLButtonElement>('joinBtn')
const callButton = element<HTMLButtonElement>('callBtn')
const hangUpButton = element<HTMLButtonElement>('hangUpBtn')
const numberInput = element<HTMLInputElement>('number')
const incomingBox = element<HTMLElement>('incoming')
const stateText = element<HTMLElement>('state')
const audioText = element<HTMLElement>('audio')

startPageTelemetry('phone', () => urlInput.value.trim())
setUpRelayUrlControls(urlInput, element('urlPresets'))
const appServerUrl = (urlFromQuery(window.location.search, ['appServerUrl']) ?? DEFAULT_APP_SERVER_URL).replace(
  /\/+$/,
  ''
)
/// The room under the app: the gateway publishes its calls in `<room>/phone`.
const ROOM = `phone-example/${pageRoomId()}`
element('room').textContent = ROOM
const me = randomName()

let joined: Joined | undefined
let call: PhoneCall | undefined
let incoming: Incoming | undefined
let audioContext: AudioContext | undefined
let audioStats: ReturnType<typeof setInterval> | undefined

joinButton.addEventListener('click', () => void join())
element<HTMLFormElement>('dialForm').addEventListener('submit', (event) => {
  event.preventDefault()
  void dial(numberInput.value.trim())
})
element('answerBtn').addEventListener('click', () => void answer())
element('declineBtn').addEventListener('click', () => void decline())
hangUpButton.addEventListener('click', () => void call?.hangUp())
for (const key of document.querySelectorAll<HTMLButtonElement>('[data-digit]')) {
  key.addEventListener('click', () => void call?.sendDtmf(key.dataset.digit!))
}

function setStatus(text: string): void {
  element('status').textContent = text
}

async function appServer<T>(path: string, body?: object): Promise<T | undefined> {
  const response = await fetch(`${appServerUrl}${path}`, {
    method: body ? 'POST' : 'GET',
    headers: body ? { 'content-type': 'application/json' } : undefined,
    body: body && JSON.stringify(body)
  })
  if (response.status === 204) {
    return undefined
  }
  const answer = await response.json()
  if (!response.ok) {
    throw new Error(`${path}: ${response.status} ${answer.error ?? ''}`)
  }
  return answer as T
}

/// Browsers start an AudioContext only on a user gesture, so every click that can lead to a call resumes it.
function resumeAudio(): AudioContext {
  audioContext ??= new AudioContext()
  void audioContext.resume()
  return audioContext
}

async function join(): Promise<void> {
  resumeAudio()
  joinButton.disabled = true
  setStatus('接続しています')
  try {
    const { token, namespace } = (await appServer<{ token: string; namespace: string[] }>('/token', { room: ROOM }))!
    const session = await MoqSession.connect(urlInput.value.trim(), { token, versions: offeredVersions() })
    joined = { session, namespace }
    setStatus('接続中')
    callButton.disabled = false
    void waitForCalls()
  } catch (error) {
    setStatus(`接続できません: ${getErrorMessage(error)}`)
    joinButton.disabled = false
  }
}

/// The page's broadcast for one call, under the room.
function localNamespace(namespace: string[]): string[] {
  return [...namespace, me, 'call', crypto.randomUUID()]
}

async function dial(to: string): Promise<void> {
  if (!joined || call) {
    return
  }
  const { session, namespace } = joined
  const local = localNamespace(namespace)
  callButton.disabled = true
  try {
    const placed = await appServer<{ call_id: string; namespace: string }>('/calls', {
      to,
      room: ROOM,
      peer: local.join('/')
    })
    await start(session, placed!.namespace.split('/'), local)
  } catch (error) {
    showState('ended', getErrorMessage(error))
    callButton.disabled = false
  }
}

/// Long-polls the app server for the calls to the app's number.
async function waitForCalls(): Promise<void> {
  while (joined) {
    try {
      const offered = await appServer<Incoming>(`/incoming?room=${encodeURIComponent(ROOM)}`)
      if (offered) {
        incoming = offered
        element('incomingFrom').textContent = offered.from
        incomingBox.hidden = false
      }
    } catch (error) {
      setStatus(`着信を待てません: ${getErrorMessage(error)}`)
      await new Promise((resolve) => setTimeout(resolve, 2_000))
    }
  }
}

async function answer(): Promise<void> {
  if (!joined || !incoming || call) {
    return
  }
  const taken = incoming
  incoming = undefined
  incomingBox.hidden = true
  resumeAudio()
  const { session, namespace } = joined
  const local = localNamespace(namespace)
  await appServer('/answer', { call_id: taken.call_id, room: ROOM, peer: local.join('/') })
  await start(session, [...namespace, 'phone', taken.call_id], local)
}

async function decline(): Promise<void> {
  if (!incoming) {
    return
  }
  const declined = incoming
  incoming = undefined
  incomingBox.hidden = true
  await appServer('/decline', { call_id: declined.call_id })
}

async function start(session: MoqSession, callNamespace: string[], local: string[]): Promise<void> {
  call = await PhoneCall.join(session, { call: callNamespace, local, audio: true, audioContext: resumeAudio() })
  hangUpButton.disabled = false
  call.listen((state, reason) => {
    showState(state, reason)
    if (state === 'ended') {
      finished()
    }
  })
  audioStats = setInterval(showAudio, AUDIO_STATS_MS)
}

function finished(): void {
  call = undefined
  clearInterval(audioStats)
  hangUpButton.disabled = true
  callButton.disabled = !joined
  audioText.textContent = ''
  audioText.dataset.kbps = '0'
}

function showState(state: PhoneCallState, reason: string | undefined): void {
  stateText.dataset.state = state
  stateText.dataset.reason = reason ?? ''
  stateText.textContent = reason ? `${STATE_LABELS[state]}（${reason}）` : STATE_LABELS[state]
}

/// The phone's audio bitrate; the E2E test reads `data-kbps` to check the call carries sound.
function showAudio(): void {
  const kbps = [...(call?.phone.sources.values() ?? [])].reduce((sum, view) => sum + view.stats().receivedKbps.audio, 0)
  audioText.dataset.kbps = kbps.toFixed(0)
  audioText.textContent = `電話の音声 ${kbps.toFixed(0)} kbps`
}
