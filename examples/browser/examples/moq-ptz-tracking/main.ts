import { MoqtClientWrapper } from '@moqt/moqtClient'
import { LivePlayer } from '@player/livePlayer'
import { SingleObjectGroupPublisher, latestRecordForPrompt, setUpDjevBotPage } from '../../utils/djevBot'
import { element, setStatus } from '../../utils/page'

const CAMERA_NAMESPACE = ['anon', 'onvif', 'client']
const VIEWER_NAMESPACE = ['anon', 'moq-ptz-tracking', 'viewer']
const TRACKER_NAMESPACE = ['anon', 'moq-ptz-tracking', 'tracker']
const PROMPT_TRACK = 'prompt'
const AUTH_INFO = ''

type Prompt = { target: string; video: string } | { target: null }

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
let promptPublisher: SingleObjectGroupPublisher | undefined
let trackingTarget: string | undefined
let sentPrompt: Prompt = { target: null }

const targetForm = element<HTMLFormElement>('targetForm')
const targetInput = element<HTMLInputElement>('target')
const stopButton = element<HTMLButtonElement>('stopBtn')
const positionLabel = element<HTMLSpanElement>('position')
const moveLabel = element<HTMLSpanElement>('move')
const viewerDelayLabel = element<HTMLSpanElement>('viewer-delay')

setUpDjevBotPage({
  session,
  logTag: 'moq-ptz-tracking',
  ownNamespace: VIEWER_NAMESPACE,
  ownTracks: [PROMPT_TRACK],
  botNamespace: TRACKER_NAMESPACE,
  afterConnect: () => player.start(CAMERA_NAMESPACE, AUTH_INFO),
  onSubscriber: async ({ requestId, trackAlias }) => {
    setPromptPublisher(new SingleObjectGroupPublisher(session, requestId, trackAlias))
    await sendPrompt(currentPrompt())
  },
  onUnsubscribe: (requestId) => {
    if (promptPublisher?.requestId === requestId) {
      setPromptPublisher(undefined)
    }
  },
  onTimelineObject: (payload) => {
    const data = latestRecordForPrompt<{
      position: TrackerRecord['position']
      pan_seconds: number
      tilt_seconds: number
    }>(payload, promptPublisher?.latestGroupId)
    if (data) {
      showRecord({ position: data.position, panSeconds: data.pan_seconds, tiltSeconds: data.tilt_seconds })
    }
  },
  onReset: async () => {
    await player.stop()
    viewerDelayLabel.textContent = ''
    setPromptPublisher(undefined)
    sentPrompt = { target: null }
  }
})
targetForm.addEventListener('submit', (event) => {
  event.preventDefault()
  targetInput.setCustomValidity(targetInput.value.trim() ? '' : '追従する対象を入力してください')
  if (targetForm.reportValidity()) {
    setTrackingTarget(targetInput.value.trim())
  }
})
stopButton.addEventListener('click', () => setTrackingTarget(undefined))

function setPromptPublisher(publisher: SingleObjectGroupPublisher | undefined): void {
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
    await promptPublisher.send(new TextEncoder().encode(JSON.stringify(prompt)))
  } catch (error) {
    console.error('[moq-ptz-tracking] prompt send failed', error)
  }
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
  const { viewerDelayMs, fastestArrivalDelayMs, bufferMs } = player.stats()
  if (viewerDelayMs === undefined) {
    viewerDelayLabel.textContent = ''
    return
  }
  const parts =
    fastestArrivalDelayMs === undefined || bufferMs === undefined
      ? ''
      : ` (arrival ${Math.round(fastestArrivalDelayMs)} ms + buffer ${Math.round(bufferMs)} ms)`
  viewerDelayLabel.textContent = `pub to sub delay ${Math.round(viewerDelayMs)} ms${parts}`
}

function formatSeconds(seconds: number): string {
  return `${seconds > 0 ? '+' : ''}${seconds.toFixed(2)} 秒`
}
