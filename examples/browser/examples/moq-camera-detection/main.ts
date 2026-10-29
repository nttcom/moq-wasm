import { MoqtClientWrapper } from '@moqt/moqtClient'
import { SubgroupWriter } from '@moqt/subgroupWriter'
import { pageRoomId } from '../../utils/botRoom'
import { type BoxEdges, boxColor, drawBoxes } from '../../utils/boxOverlay'
import { SingleObjectGroupPublisher, latestRecordForPrompt, setUpSystemOneBotPage } from '../../utils/systemOneBot'
import { timeSeededGroupId } from '../../utils/media/clock'
import { element, setStatus } from '../../utils/page'

const ROOM = pageRoomId()
const CAMERA_NAMESPACE = ['anon', 'moq-camera-detection', ROOM, 'camera']
const DETECTOR_NAMESPACE = ['anon', 'moq-camera-detection', ROOM, 'detector']
const BOT_SERVICE = {
  param: 'botUrl',
  local: 'http://127.0.0.1:8098',
  cloud: 'https://moq-camera-detection.moqt.research.skyway.io'
}
const VIDEO_TRACK = 'video'
const PROMPT_TRACK = 'prompt'
const MAX_QUESTIONS = 8
const MAX_QUESTION_LENGTH = 200
const MAX_TARGET_LENGTH = 100
const MAX_CHOICES = 9
const MAX_CHOICE_LENGTH = 40
const VIDEO_ENCODER_CONFIG = {
  codec: 'avc1.42e01f',
  width: 640,
  height: 480,
  bitrate: 500_000,
  framerate: 30,
  hardwareAcceleration: 'prefer-software'
}

class GopSender {
  private nextGroupId = timeSeededGroupId()
  private readonly writer: SubgroupWriter

  constructor(
    client: MoqtClientWrapper,
    readonly requestId: bigint,
    trackAlias: bigint
  ) {
    this.writer = new SubgroupWriter(client, trackAlias)
  }

  /// Each keyframe opens a group, so the detector, which starts a decoder at every
  /// group, always starts from a decodable picture.
  async send(chunk: EncodedVideoChunk): Promise<void> {
    if (chunk.type === 'key') {
      await this.writer.openGroup(this.nextGroupId++)
    }
    if (this.writer.openGroupId === undefined) {
      return
    }
    const payload = new Uint8Array(chunk.byteLength)
    chunk.copyTo(payload)
    await this.writer.send(payload)
  }
}

type Question = { type: 'choice'; question: string; choices: string[] } | { type: 'boxes'; target: string }
type Prompt = { questions: Question[] }
/// The choice picked (null when outside the choices), or every box found.
type Answer = string | null | BoxEdges[]

const session = new MoqtClientWrapper()
let videoSender: GopSender | undefined
let promptPublisher: SingleObjectGroupPublisher | undefined
let mediaStream: MediaStream | undefined
let encoderWorker: Worker | undefined
let sendQueue = Promise.resolve()

const preview = element<HTMLVideoElement>('preview')
const verdictList = element<HTMLUListElement>('verdicts')
const boxOverlay = element<HTMLDivElement>('boxes')
const promptForm = element<HTMLFormElement>('promptForm')
const questionList = element<HTMLDivElement>('questions')
const addQuestionButton = element<HTMLButtonElement>('addQuestionBtn')
addQuestionRow({ type: 'choice', question: '人が映っていますか？', choices: ['はい', 'いいえ'] })
let appliedPrompt = readPrompt()

setUpSystemOneBotPage({
  session,
  logTag: 'moq-camera-detection',
  ownNamespace: CAMERA_NAMESPACE,
  ownTracks: [VIDEO_TRACK, PROMPT_TRACK],
  botNamespace: DETECTOR_NAMESPACE,
  room: ROOM,
  botService: BOT_SERVICE,
  beforeConnect: async () => {
    mediaStream = await navigator.mediaDevices.getUserMedia({
      video: { width: VIDEO_ENCODER_CONFIG.width, height: VIDEO_ENCODER_CONFIG.height },
      audio: false
    })
    preview.srcObject = mediaStream
  },
  afterConnect: async () => startEncoder(mediaStream!),
  onSubscriber: async ({ trackName, requestId, trackAlias }) => {
    if (trackName === VIDEO_TRACK) {
      setVideoSender(new GopSender(session, requestId, trackAlias))
      encoderWorker?.postMessage({ type: 'forceKeyframe' })
    } else {
      promptPublisher = new SingleObjectGroupPublisher(session, requestId, trackAlias)
      await sendPrompt()
    }
  },
  onUnsubscribe: (requestId) => {
    if (videoSender?.requestId === requestId) {
      setVideoSender(undefined)
    }
    if (promptPublisher?.requestId === requestId) {
      promptPublisher = undefined
      showVerdicts(undefined)
    }
  },
  onTimelineObject: (payload) => {
    const data = latestRecordForPrompt<{ answers?: unknown }>(payload, promptPublisher?.latestGroupId)
    if (Array.isArray(data?.answers) && data.answers.length === appliedPrompt.questions.length) {
      showVerdicts(data.answers as Answer[])
    }
  },
  onReset: () => {
    setVideoSender(undefined)
    promptPublisher = undefined
    for (const track of mediaStream?.getTracks() ?? []) {
      track.stop()
    }
    mediaStream = undefined
    preview.srcObject = null
    showVerdicts(undefined)
  }
})

promptForm.addEventListener('submit', (event) => {
  event.preventDefault()
  applyPrompt()
})
addQuestionButton.addEventListener('click', () => addQuestionRow({ type: 'boxes', target: '' }))

function addQuestionRow(question: Question): void {
  const row = document.createElement('div')
  row.className = 'question-row'
  const type = document.createElement('select')
  type.setAttribute('aria-label', '質問の種類')
  type.dataset.testid = 'camera-detection-question-type'
  type.append(new Option('選択式', 'choice'), new Option('枠', 'boxes'))
  type.value = question.type
  const text = document.createElement('input')
  text.type = 'text'
  text.autocomplete = 'off'
  text.className = 'question-text'
  text.dataset.testid = 'camera-detection-question-input'
  text.value = question.type === 'choice' ? question.question : question.target
  const choices = document.createElement('input')
  choices.type = 'text'
  choices.autocomplete = 'off'
  choices.setAttribute('aria-label', '選択肢（カンマ区切り）')
  choices.placeholder = '選択肢（カンマ区切り）'
  choices.className = 'question-choices'
  choices.dataset.testid = 'camera-detection-choices-input'
  choices.value = question.type === 'choice' ? question.choices.join(', ') : ''
  const remove = document.createElement('button')
  remove.type = 'button'
  remove.textContent = '削除'
  remove.dataset.testid = 'camera-detection-remove-question-button'
  remove.addEventListener('click', () => {
    row.remove()
    updateQuestionRows()
  })
  type.addEventListener('change', () => updateQuestionRows())
  row.append(type, text, choices, remove)
  questionList.append(row)
  updateQuestionRows()
}

function updateQuestionRows(): void {
  const rows = [...questionList.children] as HTMLElement[]
  rows.forEach((row, index) => {
    const { type, text, choices, remove } = rowFields(row)
    const boxes = type.value === 'boxes'
    row.style.setProperty('--box-color', boxColor(index))
    text.placeholder = boxes ? '探すもの' : '質問'
    text.setAttribute('aria-label', text.placeholder)
    text.maxLength = boxes ? MAX_TARGET_LENGTH : MAX_QUESTION_LENGTH
    choices.hidden = boxes
    remove.disabled = rows.length === 1
  })
  addQuestionButton.disabled = rows.length >= MAX_QUESTIONS
}

function startEncoder(stream: MediaStream): void {
  encoderWorker ??= new Worker(new URL('../../utils/media/encoders/videoEncoder.ts', import.meta.url), {
    type: 'module'
  })
  encoderWorker.onmessage = (event: MessageEvent<{ type: string; chunk: EncodedVideoChunk }>) => {
    if (event.data.type === 'chunk') {
      const { chunk } = event.data
      sendQueue = sendQueue
        .then(() => videoSender?.send(chunk))
        .catch((error) => console.error('[moq-camera-detection] video send failed', error))
    }
  }
  encoderWorker.postMessage({ type: 'encoderConfig', config: VIDEO_ENCODER_CONFIG })
  const [videoTrack] = stream.getVideoTracks()
  const videoStream = new MediaStreamTrackProcessor({ track: videoTrack }).readable
  encoderWorker.postMessage({ type: 'videoStream', videoStream }, [videoStream])
}

function rowFields(row: Element) {
  return {
    type: row.querySelector('select')!,
    text: row.querySelector<HTMLInputElement>('.question-text')!,
    choices: row.querySelector<HTMLInputElement>('.question-choices')!,
    remove: row.querySelector('button')!
  }
}

function readPrompt(): Prompt {
  return { questions: [...questionList.children].map(readQuestion) }
}

function readQuestion(row: Element): Question {
  const { type, text, choices } = rowFields(row)
  if (type.value === 'boxes') {
    return { type: 'boxes', target: text.value.trim() }
  }
  return {
    type: 'choice',
    question: text.value.trim(),
    choices: choices.value
      .split(/[,、]/)
      .map((choice) => choice.trim())
      .filter((choice) => choice.length > 0)
  }
}

function validateQuestions(): boolean {
  for (const row of questionList.children) {
    const { text, choices } = rowFields(row)
    const question = readQuestion(row)
    if (question.type === 'boxes') {
      text.setCustomValidity(question.target ? '' : '探すものを入力してください')
      choices.setCustomValidity('')
      continue
    }
    const validChoices =
      question.choices.length >= 2 &&
      question.choices.length <= MAX_CHOICES &&
      question.choices.every((choice) => choice.length <= MAX_CHOICE_LENGTH)
    text.setCustomValidity(question.question ? '' : '質問を入力してください')
    choices.setCustomValidity(validChoices ? '' : `選択肢は 2〜${MAX_CHOICES} 個、各 ${MAX_CHOICE_LENGTH} 文字以内`)
  }
  return promptForm.reportValidity()
}

function applyPrompt(): void {
  if (!validateQuestions()) {
    return
  }
  appliedPrompt = readPrompt()
  void sendPrompt()
}

async function sendPrompt(): Promise<void> {
  if (!promptPublisher) {
    return
  }
  showVerdicts(undefined)
  try {
    await promptPublisher.send(new TextEncoder().encode(JSON.stringify(appliedPrompt)))
  } catch (error) {
    console.error('[moq-camera-detection] prompt send failed', error)
  }
}

function setVideoSender(sender: GopSender | undefined): void {
  videoSender = sender
  setStatus('detector-status', sender ? 'bot 接続済み' : 'bot 未接続', sender ? 'ok' : 'idle')
}

/// One badge per applied question, and the boxes of every boxes question over the preview.
function showVerdicts(answers: Answer[] | undefined): void {
  const { questions } = appliedPrompt
  verdictList.replaceChildren(
    ...questions.map((question, index) => {
      const answer = answers?.[index]
      const badge = document.createElement('li')
      badge.className = 'verdict-badge'
      badge.dataset.testid = 'camera-detection-verdict'
      badge.style.setProperty('--box-color', boxColor(index))
      badge.dataset.verdict = answer === undefined ? 'pending' : answer === null ? 'unknown' : 'answer'
      badge.textContent =
        answer === undefined
          ? '判定待ち'
          : answer === null
            ? '判定できません'
            : Array.isArray(answer)
              ? `${question.type === 'boxes' ? question.target : ''} ${answer.length}`
              : answer
      return badge
    })
  )
  drawBoxes(
    boxOverlay,
    questions.flatMap((question, index) => {
      const answer = answers?.[index]
      return question.type === 'boxes' && Array.isArray(answer)
        ? answer.map((edges) => ({ edges, label: question.target, color: boxColor(index) }))
        : []
    }),
    'camera-detection-box'
  )
}
