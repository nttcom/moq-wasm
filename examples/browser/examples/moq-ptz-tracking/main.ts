import { type IncomingSubscribeContext, MoqtClientWrapper, RequestErrorCode } from '@moqt/moqtClient'
import { LivePlayer } from '@player/livePlayer'
import type { MOQTClient, SubgroupObjectMessage } from '../../pkg/moqt'
import { sendSingleObjectGroup } from '../../utils/media/singleObjectGroup'
import { DjevStatusView } from '../../utils/djevStatus'
import { DEFAULT_LOCAL_RELAY_A_URL, configureRelayUrlControls } from '../../utils/relayPresets'
import { type StatusState, element, getErrorMessage, setStatus } from '../media/common'

const CAMERA_NAMESPACE = ['anon', 'onvif', 'client']
const VIEWER_NAMESPACE = ['anon', 'moq-ptz-tracking', 'viewer']
const TRACKER_NAMESPACE = ['anon', 'moq-ptz-tracking', 'tracker']
const PROMPT_TRACK = 'prompt'
const EVENT_TIMELINE_TRACK = 'eventtimeline'
const AUTH_INFO = ''
const LARGEST_OBJECT_FILTER = 0x2
type Prompt = { target: string; video: string } | { target: null }

class PromptPublisher {
  /// The relay isolates a track whose publisher repeats a location, so a
  /// publisher that comes back must not restart its group ids at zero.
  private nextGroupId = BigInt(Date.now()) * 1_000n
  latestGroupId: bigint | undefined

  constructor(
    private readonly client: MOQTClient,
    readonly requestId: bigint,
    private readonly trackAlias: bigint
  ) {}

  /// One group per prompt, because the tracker reads every group from its first object.
  async send(prompt: Prompt): Promise<void> {
    const groupId = this.nextGroupId++
    this.latestGroupId = groupId
    const payload = new TextEncoder().encode(JSON.stringify(prompt))
    await sendSingleObjectGroup(this.client, this.trackAlias, groupId, payload)
  }
}

const session = new MoqtClientWrapper()
const player = new LivePlayer({
  client: session,
  container: element('stage'),
  callbacks: {
    onStateChange: followSelectedVideoTrack,
    onLiveFrame: renderViewerDelay,
    onLog: (level, message) => console[level](`[moq-ptz-tracking] ${message}`)
  }
})
let promptPublisher: PromptPublisher | undefined
let trackingTarget: string | undefined
let sentPrompt: Prompt = { target: null }

const urlInput = element<HTMLInputElement>('url')
const joinButton = element<HTMLButtonElement>('joinBtn')
const leaveButton = element<HTMLButtonElement>('leaveBtn')
const targetForm = element<HTMLFormElement>('targetForm')
const targetInput = element<HTMLInputElement>('target')
const stopButton = element<HTMLButtonElement>('stopBtn')
const positionLabel = element<HTMLSpanElement>('position')
const moveLabel = element<HTMLSpanElement>('move')
const viewerDelayLabel = element<HTMLSpanElement>('viewer-delay')
const djevStatus = new DjevStatusView(element('djev-status'))

configureRelayUrlControls({ defaultUrl: DEFAULT_LOCAL_RELAY_A_URL })

joinButton.addEventListener('click', () => void join())
leaveButton.addEventListener('click', () => void leave())
targetForm.addEventListener('submit', (event) => {
  event.preventDefault()
  targetInput.setCustomValidity(targetInput.value.trim() ? '' : '追従する対象を入力してください')
  if (targetForm.reportValidity()) {
    setTrackingTarget(targetInput.value.trim())
  }
})
stopButton.addEventListener('click', () => setTrackingTarget(undefined))

async function join(): Promise<void> {
  joinButton.disabled = true
  setStatus('connection-status', '接続中', 'review')
  try {
    await session.connect(urlInput.value.trim())
    session.setOnConnectionClosedHandler(() => void resetSession('切断されました', 'error'))
    session.setOnIncomingSubscribeHandler(acceptTrackerSubscriber)
    session.setOnIncomingUnsubscribeHandler((requestId) => {
      if (promptPublisher?.requestId === requestId) {
        setPromptPublisher(undefined)
      }
    })
    session.setOnPublishNamespaceHandler(async ({ publishNamespace, respondOk }) => {
      await respondOk()
      if (sameNamespace(publishNamespace.trackNamespace, TRACKER_NAMESPACE)) {
        await subscribeTrackerRecords()
        await djevStatus
          .follow(session, TRACKER_NAMESPACE)
          .catch((error) => console.warn('[moq-ptz-tracking] djev status subscribe failed', error))
      }
    })
    await session.publishNamespace(VIEWER_NAMESPACE, AUTH_INFO)
    await session.subscribeNamespace(TRACKER_NAMESPACE, AUTH_INFO)
    await player.start(CAMERA_NAMESPACE, AUTH_INFO)
    setStatus('connection-status', '接続済み', 'ok')
    leaveButton.disabled = false
  } catch (error) {
    await session.disconnect()
    await resetSession(`接続に失敗しました: ${getErrorMessage(error)}`, 'error')
  }
}

async function leave(): Promise<void> {
  await session.disconnect()
  await resetSession('未接続', 'idle')
}

async function resetSession(connectionText: string, connectionState: StatusState): Promise<void> {
  await player.stop()
  viewerDelayLabel.textContent = ''
  djevStatus.reset()
  setPromptPublisher(undefined)
  sentPrompt = { target: null }
  setStatus('connection-status', connectionText, connectionState)
  joinButton.disabled = false
  leaveButton.disabled = true
}

async function acceptTrackerSubscriber({
  subscribe,
  isSuccess,
  code,
  respondOk,
  respondError
}: IncomingSubscribeContext): Promise<void> {
  if (!isSuccess) {
    await respondError(BigInt(code), 'subscribe rejected')
    return
  }
  if (!sameNamespace(subscribe.trackNamespace, VIEWER_NAMESPACE) || subscribe.trackName !== PROMPT_TRACK) {
    await respondError(RequestErrorCode.TrackDoesNotExist, 'unknown track')
    return
  }
  const trackAlias = await respondOk()
  const client = session.getRawClient()
  if (client) {
    setPromptPublisher(new PromptPublisher(client, subscribe.requestId, trackAlias))
    await sendPrompt(currentPrompt())
  }
}

function setPromptPublisher(publisher: PromptPublisher | undefined): void {
  promptPublisher = publisher
  setStatus('tracker-status', publisher ? 'bot 接続済み' : 'bot 未接続', publisher ? 'ok' : 'idle')
  showRecord(undefined)
}

function setTrackingTarget(target: string | undefined): void {
  trackingTarget = target
  stopButton.disabled = target === undefined
  void sendPrompt(currentPrompt())
}

function currentPrompt(): Prompt {
  const video = player.state.selectedVideoTrack
  return trackingTarget && video ? { target: trackingTarget, video } : { target: null }
}

/// The bot watches the video track named in the prompt, so a prompt follows the
/// player to the track it plays.
function followSelectedVideoTrack(): void {
  const prompt = currentPrompt()
  if (JSON.stringify(prompt) !== JSON.stringify(sentPrompt)) {
    void sendPrompt(prompt)
  }
}

async function sendPrompt(prompt: Prompt): Promise<void> {
  sentPrompt = prompt
  showRecord(undefined)
  if (!promptPublisher) {
    return
  }
  try {
    await promptPublisher.send(prompt)
  } catch (error) {
    console.error('[moq-ptz-tracking] prompt send failed', error)
  }
}

async function subscribeTrackerRecords(): Promise<void> {
  try {
    const { subscribeOk } = await session.subscribe(TRACKER_NAMESPACE, EVENT_TIMELINE_TRACK, AUTH_INFO, {
      filterType: LARGEST_OBJECT_FILTER,
      forward: true
    })
    session.setOnSubgroupObjectHandler(subscribeOk.trackAlias, (_groupId, object) => applyRecords(object))
  } catch (error) {
    console.warn('[moq-ptz-tracking] event timeline subscribe failed', error)
  }
}

/// draft-ietf-moq-msf-01 §8.1: each event timeline object is a JSON array of records; the
/// newest one, last in the array, carries the latest result.
function applyRecords(object: SubgroupObjectMessage): void {
  if (object.objectStatus !== undefined) {
    return
  }
  const data = JSON.parse(new TextDecoder().decode(new Uint8Array(object.objectPayload))).at(-1)?.data
  const [promptGroupId] = Array.isArray(data?.prompt) ? data.prompt : []
  if (!Number.isInteger(promptGroupId) || BigInt(promptGroupId) !== promptPublisher?.latestGroupId) {
    return
  }
  showRecord({ position: data.position, panSeconds: data.pan_seconds, tiltSeconds: data.tilt_seconds })
}

type TrackerRecord = {
  position: [number, number] | 'not visible' | null
  panSeconds: number
  tiltSeconds: number
}

function showRecord(record: TrackerRecord | undefined): void {
  positionLabel.dataset.position = !record ? 'pending' : record.position === null ? 'unknown' : 'answer'
  positionLabel.textContent = !record ? pendingText() : positionText(record.position)
  moveLabel.textContent =
    record && (record.panSeconds !== 0 || record.tiltSeconds !== 0)
      ? `パン ${formatSeconds(record.panSeconds)}・チルト ${formatSeconds(record.tiltSeconds)}`
      : ''
}

function positionText(position: TrackerRecord['position']): string {
  if (position === null) {
    return '判定できません'
  }
  if (position === 'not visible') {
    return '見つかりません'
  }
  const [x, y] = position
  return `横 ${Math.round(x * 100)}%・縦 ${Math.round(y * 100)}%`
}

function pendingText(): string {
  if (trackingTarget === undefined) {
    return '停止中'
  }
  return sentPrompt.target === null ? '映像を待っています' : '判定待ち'
}

function renderViewerDelay(): void {
  const { viewerDelayMs } = player.stats()
  viewerDelayLabel.textContent = viewerDelayMs === undefined ? '' : `delay ${Math.round(viewerDelayMs)} ms`
}

function formatSeconds(seconds: number): string {
  return `${seconds > 0 ? '+' : ''}${seconds.toFixed(2)} 秒`
}

function sameNamespace(left: string[], right: string[]): boolean {
  return left.join('/') === right.join('/')
}
