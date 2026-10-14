import { MoqtClientWrapper } from '@moqt/moqtClient'
import { SubgroupWriter } from '@moqt/subgroupWriter'
import { SingleObjectGroupPublisher, latestRecordForPrompt, setUpDjevBotPage } from '../../utils/djevBot'
import { timeSeededGroupId } from '../../utils/media/clock'
import { element, setStatus } from '../../utils/page'

const CAMERA_NAMESPACE = ['anon', 'moq-camera-detection', 'camera']
const DETECTOR_NAMESPACE = ['anon', 'moq-camera-detection', 'detector']
const VIDEO_TRACK = 'video'
const PROMPT_TRACK = 'prompt'
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

type Prompt = { question: string; choices: string[] }

const session = new MoqtClientWrapper()
let videoSender: GopSender | undefined
let promptPublisher: SingleObjectGroupPublisher | undefined
let mediaStream: MediaStream | undefined
let encoderWorker: Worker | undefined
let sendQueue = Promise.resolve()

const preview = element<HTMLVideoElement>('preview')
const verdictLabel = element<HTMLSpanElement>('verdict')
const promptForm = element<HTMLFormElement>('promptForm')
const questionInput = element<HTMLInputElement>('question')
const choicesInput = element<HTMLInputElement>('choices')
let appliedPrompt = readPrompt()

setUpDjevBotPage({
  session,
  logTag: 'moq-camera-detection',
  ownNamespace: CAMERA_NAMESPACE,
  ownTracks: [VIDEO_TRACK, PROMPT_TRACK],
  botNamespace: DETECTOR_NAMESPACE,
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
      showVerdict(undefined)
    }
  },
  onTimelineObject: (payload) => {
    const data = latestRecordForPrompt<{ answer?: unknown }>(payload, promptPublisher?.latestGroupId)
    if (data && (typeof data.answer === 'string' || data.answer === null)) {
      showVerdict(data.answer)
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
    showVerdict(undefined)
  }
})

promptForm.addEventListener('submit', (event) => {
  event.preventDefault()
  applyPrompt()
})

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

function readPrompt(): Prompt {
  return {
    question: questionInput.value.trim(),
    choices: choicesInput.value
      .split(/[,、]/)
      .map((choice) => choice.trim())
      .filter((choice) => choice.length > 0)
  }
}

function applyPrompt(): void {
  const prompt = readPrompt()
  const { question, choices } = prompt
  const validChoices =
    choices.length >= 2 && choices.length <= MAX_CHOICES && choices.every((c) => c.length <= MAX_CHOICE_LENGTH)
  questionInput.setCustomValidity(question ? '' : '質問を入力してください')
  choicesInput.setCustomValidity(validChoices ? '' : `選択肢は 2〜${MAX_CHOICES} 個、各 ${MAX_CHOICE_LENGTH} 文字以内`)
  if (!promptForm.reportValidity()) {
    return
  }
  appliedPrompt = prompt
  void sendPrompt()
}

async function sendPrompt(): Promise<void> {
  if (!promptPublisher) {
    return
  }
  showVerdict(undefined)
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

function showVerdict(answer: string | null | undefined): void {
  verdictLabel.dataset.verdict = answer === undefined ? 'pending' : answer === null ? 'unknown' : 'answer'
  verdictLabel.textContent = answer === undefined ? '判定待ち' : answer === null ? '判定できません' : answer
}
