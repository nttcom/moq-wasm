import type { TrackWriter } from 'moq-wasm'
import { LivePlayer } from 'moq-wasm/media/player/livePlayer'
import { latestRecordForPrompt, onEachSubscriber, setUpSystemOneBotPage } from '../../utils/systemOneBot'
import { type BoxEdges, boxColor, drawBoxes } from '../../utils/boxOverlay'
import { element, setStatus } from '../../utils/page'
import 'moq-wasm/media/player.css'

const CAMERA_NAMESPACE = ['anon', 'onvif', 'client']
const VIEWER_NAMESPACE = ['anon', 'moq-ptz-tracking', 'viewer']
const TRACKER_NAMESPACE = ['anon', 'moq-ptz-tracking', 'tracker']
// There is one ONVIF camera, so the bot service runs this one room only.
const ROOM = 'onvif'
const BOT_SERVICE = {
  param: 'botUrl',
  local: 'http://127.0.0.1:8099',
  cloud: 'https://moq-ptz-tracking.moqt.research.skyway.io'
}
const PROMPT_TRACK = 'prompt'
const MAX_TARGETS = 16

type Prompt = { targets: string[]; video?: string }
/// [index into the prompt's targets, edges]
type TargetBox = [number, BoxEdges]

const player = new LivePlayer({
  container: element('stage'),
  callbacks: {
    onStateChange: followSelectedVideoTrack,
    onLiveFrame: renderViewerDelay,
    onLog: (level, message) => console[level](`[moq-ptz-tracking] ${message}`)
  }
})
let promptWriter: TrackWriter | undefined
let promptGroupId: bigint | undefined
let trackingTargets: string[] = []
let sentPrompt: Prompt = { targets: [] }

const targetForm = element<HTMLFormElement>('targetForm')
const targetList = element<HTMLDivElement>('targets')
const addTargetButton = element<HTMLButtonElement>('addTargetBtn')
const stopButton = element<HTMLButtonElement>('stopBtn')
const boxOverlay = element<HTMLDivElement>('boxes')
const positionLabel = element<HTMLSpanElement>('position')
const moveLabel = element<HTMLSpanElement>('move')
const viewerDelayLabel = element<HTMLSpanElement>('viewer-delay')

setUpSystemOneBotPage({
  logTag: 'moq-ptz-tracking',
  ownNamespace: VIEWER_NAMESPACE,
  botNamespace: TRACKER_NAMESPACE,
  room: ROOM,
  botService: BOT_SERVICE,
  onConnect: async (session) => {
    promptWriter = await session.publishTrack(VIEWER_NAMESPACE, PROMPT_TRACK)
    onEachSubscriber(promptWriter, async () => {
      setTrackerStatus(true)
      await sendPrompt(currentPrompt())
    })
  },
  afterConnect: (session) => player.start(session, CAMERA_NAMESPACE),
  onBotLeft: () => setTrackerStatus(false),
  onTimelineObject: (payload) => {
    const data = latestRecordForPrompt<{
      boxes: TrackerRecord['boxes']
      pan_seconds: number
      tilt_seconds: number
    }>(payload, promptGroupId)
    if (data) {
      showRecord({ boxes: data.boxes, panSeconds: data.pan_seconds, tiltSeconds: data.tilt_seconds })
    }
  },
  onReset: async () => {
    await player.stop()
    viewerDelayLabel.textContent = ''
    promptWriter = undefined
    promptGroupId = undefined
    setTrackerStatus(false)
    sentPrompt = { targets: [] }
  }
})
addTargetRow('人')
targetForm.addEventListener('submit', (event) => {
  event.preventDefault()
  const inputs = [...targetList.querySelectorAll('input')]
  const targets = [...new Set(inputs.map((input) => input.value.trim()).filter((text) => text))]
  inputs[0].setCustomValidity(targets.length > 0 ? '' : '追従する対象を入力してください')
  if (targetForm.reportValidity()) {
    setTrackingTargets(targets)
  }
})
addTargetButton.addEventListener('click', () => addTargetRow('').focus())
stopButton.addEventListener('click', () => setTrackingTargets([]))

function addTargetRow(value: string): HTMLInputElement {
  const row = document.createElement('div')
  row.className = 'target-row'
  const input = document.createElement('input')
  input.type = 'text'
  input.value = value
  input.maxLength = 100
  input.autocomplete = 'off'
  input.setAttribute('aria-label', '追従する対象')
  input.dataset.testid = 'ptz-tracking-target-input'
  const remove = document.createElement('button')
  remove.type = 'button'
  remove.textContent = '削除'
  remove.dataset.testid = 'ptz-tracking-remove-target-button'
  remove.addEventListener('click', () => {
    row.remove()
    updateTargetRows()
  })
  row.append(input, remove)
  targetList.append(row)
  updateTargetRows()
  return input
}

function updateTargetRows(): void {
  const rows = [...targetList.children] as HTMLElement[]
  rows.forEach((row, index) => {
    row.style.setProperty('--box-color', boxColor(index))
    row.querySelector('button')!.disabled = rows.length === 1
  })
  addTargetButton.disabled = rows.length >= MAX_TARGETS
}

function setTrackerStatus(joined: boolean): void {
  setStatus('tracker-status', joined ? 'bot 接続済み' : 'bot 未接続', joined ? 'ok' : 'idle')
  showRecord(undefined)
}

function setTrackingTargets(targets: string[]): void {
  trackingTargets = targets
  stopButton.disabled = targets.length === 0
  void sendPrompt(currentPrompt())
}

function currentPrompt(): Prompt {
  const video = player.state.selectedVideoTrack
  return trackingTargets.length > 0 && video ? { targets: trackingTargets, video } : { targets: [] }
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
  if (!promptWriter) {
    return
  }
  try {
    promptGroupId = await promptWriter.writeGroup(new TextEncoder().encode(JSON.stringify(prompt)))
  } catch (error) {
    console.error('[moq-ptz-tracking] prompt send failed', error)
  }
}

type TrackerRecord = {
  boxes: TargetBox[] | 'not visible'
  panSeconds: number
  tiltSeconds: number
}

function showRecord(record: TrackerRecord | undefined): void {
  const boxes = record && record.boxes !== 'not visible' ? record.boxes : []
  positionLabel.dataset.position = !record ? 'pending' : boxes.length === 0 ? 'unknown' : 'answer'
  positionLabel.textContent = !record ? pendingText() : boxes.length === 0 ? '見つかりません' : countText(boxes)
  drawTargetBoxes(boxes)
  moveLabel.textContent =
    record && (record.panSeconds !== 0 || record.tiltSeconds !== 0)
      ? `パン ${formatSeconds(record.panSeconds)}・チルト ${formatSeconds(record.tiltSeconds)}`
      : ''
}

function countText(boxes: TargetBox[]): string {
  return sentPrompt.targets
    .map((target, index) => `${target} ${boxes.filter(([found]) => found === index).length}`)
    .join('・')
}

function drawTargetBoxes(boxes: TargetBox[]): void {
  drawBoxes(
    boxOverlay,
    boxes.map(([target, edges]) => ({ edges, label: sentPrompt.targets[target] ?? '', color: boxColor(target) })),
    'ptz-tracking-box'
  )
}

function pendingText(): string {
  if (trackingTargets.length === 0) {
    return '停止中'
  }
  return sentPrompt.targets.length === 0 ? '映像を待っています' : '判定待ち'
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
